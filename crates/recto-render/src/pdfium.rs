//! The PDFium seam, and the only `unsafe` in this crate.
//!
//! **Raw FFI, not `pdfium-render`.** The spike used `pdfium-render` 0.9.5 only for its bindings, because `PdfDocument::handle()` is
//! crate-private and its high-level loaders can't reach `FPDF_LoadCustomDocument` with a `pread` callback, the loader the worker needs
//! (spike §3). Taking the crate for function pointers alone costs 24 crates and a much larger binary, for what is 33 signatures here.
//! Those are copied from the headers of the version `tools/pdfium.sh` pins by digest: chromium/8125, non-V8, non-XFA. Every one was compared with those
//! headers by hand on 2026-11-06.
//!
//! **No digest check at load.** The script checks the file when it's fetched; nothing checks it each time a worker starts, because hashing 7 MB per spawn
//! would cost more than the spawn. Code signing covers it in the app. `Api::load` catches a library missing a function, not a changed one.
//!
//! A page's contents are read in the `contents` module under this one, which is part of this seam.
//!
//! **Not thread-safe.** PDFium holds global state. So there's one library per process, one live document ([`PdfiumBackend::open`]
//! refuses a second), and the backend is `!Send`. Parallelism comes from more worker processes, which measured better anyway (spike §4).

#![allow(unsafe_code, non_snake_case, non_camel_case_types, clippy::too_many_arguments)]

use crate::{Cancel, PageChars, PagePoints, RenderBackend, RenderError, TextOptions, TileOutcome, classify_bgrx};
use recto_platform::isolate::Library;
use recto_proto::contents::{ContentsJob, Mat};
use recto_proto::{CharBox, CharRecord, FailCode, MAX_PAGE_CHARS, MAX_PAGE_COUNT, PageFrame, TileKind, TileRect, device_size, merge_kinds, region_tiles};
use std::cell::Cell;
use std::ffi::{CString, c_double, c_float, c_int, c_uchar, c_uint, c_ulong, c_void};
use std::fs::File;
use std::marker::PhantomData;
use std::path::Path;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

type FPDF_DOCUMENT = *mut c_void;
type FPDF_PAGE = *mut c_void;
type FPDF_BITMAP = *mut c_void;
type FPDF_TEXTPAGE = *mut c_void;
type FPDF_FORMHANDLE = *mut c_void;
type FPDF_PAGEOBJECT = *mut c_void;
type FPDF_PAGEOBJECTMARK = *mut c_void;
type FPDF_CLIPPATH = *mut c_void;
type FPDF_PATHSEGMENT = *mut c_void;
type FPDF_GLYPHPATH = *mut c_void;
type FPDF_FONT = *mut c_void;
type FPDF_ANNOTATION = *mut c_void;
type FPDF_BOOL = c_int;

mod contents;
use contents::Walk;

/// `fpdfview.h`: a transform.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct FS_MATRIX {
    a: c_float,
    b: c_float,
    c: c_float,
    d: c_float,
    e: c_float,
    f: c_float,
}

/// `fpdfview.h`: a rectangle.
#[repr(C)]
struct FS_RECTF {
    left: c_float,
    top: c_float,
    right: c_float,
    bottom: c_float,
}

/// `fpdf_edit.h`: what's known about an image.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct FPDF_IMAGEOBJ_METADATA {
    width: c_uint,
    height: c_uint,
    horizontal_dpi: c_float,
    vertical_dpi: c_float,
    bits_per_pixel: c_uint,
    colorspace: c_int,
    marked_content_id: c_int,
}

/// `fpdfview.h`: read callback for `FPDF_LoadCustomDocument`.
#[repr(C)]
struct FPDF_FILEACCESS {
    m_FileLen: c_ulong,
    m_GetBlock: unsafe extern "C" fn(param: *mut c_void, position: c_ulong, buf: *mut c_uchar, size: c_ulong) -> c_int,
    m_Param: *mut c_void,
}

/// `fpdf_progressive.h`: the pause callback.
#[repr(C)]
struct IFSDK_PAUSE {
    version: c_int,
    NeedToPauseNow: unsafe extern "C" fn(this: *mut IFSDK_PAUSE) -> FPDF_BOOL,
    user: *mut c_void,
}

/// `fpdf_formfill.h`: the form-fill callbacks, all left null. There is no form engine here: forms are drawn, never run (SPEC §6), so
/// only `version` is set. Version 2 has under 50 pointers; the slack keeps the struct at least as long as PDFium reads.
#[repr(C)]
struct FPDF_FORMFILLINFO {
    version: c_int,
    callbacks: [*mut c_void; 64],
}

/// `fpdfview.h`: pixel formats and flags, and `fpdf_progressive.h`: render status.
#[allow(non_upper_case_globals)]
const FPDFBitmap_BGRx: c_int = 3;
const FPDF_ANNOT: c_int = 0x01;
const FPDF_RENDER_TOBECONTINUED: c_int = 1;
const FPDF_RENDER_DONE: c_int = 2;
/// `FPDF_GetLastError` codes.
const FPDF_ERR_FILE: c_ulong = 2;
const FPDF_ERR_FORMAT: c_ulong = 3;
const FPDF_ERR_PASSWORD: c_ulong = 4;
const FPDF_ERR_SECURITY: c_ulong = 5;
/// `fpdf_doc.h`'s permission bit for copying or extracting text. The table in the PDF standard, 7.6.4.2, bit 5.
const PERMISSION_COPY: c_ulong = 1 << 4;
/// `fpdf_edit.h`: object types.
const FPDF_PAGEOBJ_IMAGE: c_int = 3;
const FPDF_PAGEOBJ_FORM: c_int = 5;
/// How far the look for large images goes. A hostile page can hold millions of objects and forms inside forms, and it's only a hint.
const MAX_OBJECTS_LOOKED_AT: usize = 20_000;
const MAX_FORM_DEPTH: u32 = 8;
/// The background of every page.
const PAPER_WHITE: c_ulong = 0xFFFF_FFFF;

/// Declares the functions this seam uses, resolved from the library by name at load.
macro_rules! pdfium_api {
    ($($name:ident: fn($($arg:ty),*) $(-> $ret:ty)?;)*) => {
        struct Api { $($name: unsafe extern "C" fn($($arg),*) $(-> $ret)?,)* }

        impl Api {
            fn load(library: &Library) -> Option<Self> {
                Some(Self { $(
                    // SAFETY: this signature is copied from the headers of the version `tools/pdfium.sh` pins, and the caller names that library:
                    // it isn't re-checked here, see the module's notes. A pointer to a function and a `fn` have the same representation.
                    $name: unsafe {
                        std::mem::transmute::<*mut c_void, unsafe extern "C" fn($($arg),*) $(-> $ret)?>(
                            library.symbol(&CString::new(stringify!($name)).ok()?)?.as_ptr(),
                        )
                    },
                )* })
            }
        }
    };
}

pdfium_api! {
    FPDF_InitLibrary: fn();
    FPDF_GetLastError: fn() -> c_ulong;
    FPDF_LoadCustomDocument: fn(*mut FPDF_FILEACCESS, *const std::ffi::c_char) -> FPDF_DOCUMENT;
    FPDF_CloseDocument: fn(FPDF_DOCUMENT);
    FPDF_GetPageCount: fn(FPDF_DOCUMENT) -> c_int;
    FPDF_GetPageSizeByIndex: fn(FPDF_DOCUMENT, c_int, *mut c_double, *mut c_double) -> c_int;
    FPDF_LoadPage: fn(FPDF_DOCUMENT, c_int) -> FPDF_PAGE;
    FPDF_ClosePage: fn(FPDF_PAGE);
    FPDF_GetDocPermissions: fn(FPDF_DOCUMENT) -> c_ulong;
    FPDF_PageToDevice: fn(FPDF_PAGE, c_int, c_int, c_int, c_int, c_int, c_double, c_double, *mut c_int, *mut c_int) -> FPDF_BOOL;
    FPDFBitmap_CreateEx: fn(c_int, c_int, c_int, *mut c_void, c_int) -> FPDF_BITMAP;
    FPDFBitmap_FillRect: fn(FPDF_BITMAP, c_int, c_int, c_int, c_int, c_ulong) -> FPDF_BOOL;
    FPDFBitmap_Destroy: fn(FPDF_BITMAP);
    FPDF_RenderPageBitmap_Start: fn(FPDF_BITMAP, FPDF_PAGE, c_int, c_int, c_int, c_int, c_int, c_int, *mut IFSDK_PAUSE) -> c_int;
    FPDF_RenderPage_Continue: fn(FPDF_PAGE, *mut IFSDK_PAUSE) -> c_int;
    FPDF_RenderPage_Close: fn(FPDF_PAGE);
    FPDFDOC_InitFormFillEnvironment: fn(FPDF_DOCUMENT, *mut FPDF_FORMFILLINFO) -> FPDF_FORMHANDLE;
    FPDFDOC_ExitFormFillEnvironment: fn(FPDF_FORMHANDLE);
    FORM_OnAfterLoadPage: fn(FPDF_PAGE, FPDF_FORMHANDLE);
    FORM_OnBeforeClosePage: fn(FPDF_PAGE, FPDF_FORMHANDLE);
    FPDF_FFLDraw: fn(FPDF_FORMHANDLE, FPDF_BITMAP, FPDF_PAGE, c_int, c_int, c_int, c_int, c_int, c_int);
    FPDFPage_CountObjects: fn(FPDF_PAGE) -> c_int;
    FPDFPage_GetObject: fn(FPDF_PAGE, c_int) -> FPDF_PAGEOBJECT;
    FPDFPageObj_GetType: fn(FPDF_PAGEOBJECT) -> c_int;
    FPDFFormObj_CountObjects: fn(FPDF_PAGEOBJECT) -> c_int;
    FPDFFormObj_GetObject: fn(FPDF_PAGEOBJECT, c_ulong) -> FPDF_PAGEOBJECT;
    FPDFImageObj_GetImagePixelSize: fn(FPDF_PAGEOBJECT, *mut c_uint, *mut c_uint) -> FPDF_BOOL;
    FPDFPageObj_GetBounds: fn(FPDF_PAGEOBJECT, *mut std::ffi::c_float, *mut std::ffi::c_float, *mut std::ffi::c_float, *mut std::ffi::c_float) -> FPDF_BOOL;
    FPDFText_LoadPage: fn(FPDF_PAGE) -> FPDF_TEXTPAGE;
    FPDFText_ClosePage: fn(FPDF_TEXTPAGE);
    FPDFText_CountChars: fn(FPDF_TEXTPAGE) -> c_int;
    FPDFText_GetUnicode: fn(FPDF_TEXTPAGE, c_int) -> c_uint;
    FPDFText_IsHyphen: fn(FPDF_TEXTPAGE, c_int) -> c_int;
    FPDFText_IsGenerated: fn(FPDF_TEXTPAGE, c_int) -> c_int;
    FPDFText_GetCharBox: fn(FPDF_TEXTPAGE, c_int, *mut c_double, *mut c_double, *mut c_double, *mut c_double) -> FPDF_BOOL;
    FPDFText_GetCharOrigin: fn(FPDF_TEXTPAGE, c_int, *mut c_double, *mut c_double) -> FPDF_BOOL;
    FPDFText_GetTextObject: fn(FPDF_TEXTPAGE, c_int) -> FPDF_PAGEOBJECT;
    FPDF_GetPageBoundingBox: fn(FPDF_PAGE, *mut FS_RECTF) -> FPDF_BOOL;
    FPDFPage_GetRotation: fn(FPDF_PAGE) -> c_int;
    FPDFPageObj_HasTransparency: fn(FPDF_PAGEOBJECT) -> FPDF_BOOL;
    FPDFPageObj_SetIsActive: fn(FPDF_PAGEOBJECT, FPDF_BOOL) -> FPDF_BOOL;
    FPDFPageObj_GetMatrix: fn(FPDF_PAGEOBJECT, *mut FS_MATRIX) -> FPDF_BOOL;
    FPDFPageObj_SetMatrix: fn(FPDF_PAGEOBJECT, *const FS_MATRIX) -> FPDF_BOOL;
    FPDFPageObj_CountMarks: fn(FPDF_PAGEOBJECT) -> c_int;
    FPDFPageObj_GetMark: fn(FPDF_PAGEOBJECT, c_ulong) -> FPDF_PAGEOBJECTMARK;
    FPDFPageObjMark_GetName: fn(FPDF_PAGEOBJECTMARK, *mut u16, c_ulong, *mut c_ulong) -> FPDF_BOOL;
    FPDFPageObjMark_GetParamStringValue: fn(FPDF_PAGEOBJECTMARK, *const std::ffi::c_char, *mut u16, c_ulong, *mut c_ulong) -> FPDF_BOOL;
    FPDFPageObj_AddExistingMark: fn(FPDF_PAGEOBJECT, FPDF_PAGEOBJECTMARK) -> FPDF_BOOL;
    FPDFPageObj_CreateNewRect: fn(c_float, c_float, c_float, c_float) -> FPDF_PAGEOBJECT;
    FPDFPageObj_Destroy: fn(FPDF_PAGEOBJECT);
    FPDFPath_SetDrawMode: fn(FPDF_PAGEOBJECT, c_int, FPDF_BOOL) -> FPDF_BOOL;
    FPDFPage_InsertObject: fn(FPDF_PAGE, FPDF_PAGEOBJECT);
    FPDFPage_RemoveObject: fn(FPDF_PAGE, FPDF_PAGEOBJECT) -> FPDF_BOOL;
    FPDFPageObj_GetFillColor: fn(FPDF_PAGEOBJECT, *mut c_uint, *mut c_uint, *mut c_uint, *mut c_uint) -> FPDF_BOOL;
    FPDFPageObj_SetFillColor: fn(FPDF_PAGEOBJECT, c_uint, c_uint, c_uint, c_uint) -> FPDF_BOOL;
    FPDFPageObj_GetStrokeColor: fn(FPDF_PAGEOBJECT, *mut c_uint, *mut c_uint, *mut c_uint, *mut c_uint) -> FPDF_BOOL;
    FPDFPageObj_GetStrokeWidth: fn(FPDF_PAGEOBJECT, *mut c_float) -> FPDF_BOOL;
    FPDFPageObj_GetLineJoin: fn(FPDF_PAGEOBJECT) -> c_int;
    FPDFPageObj_GetLineCap: fn(FPDF_PAGEOBJECT) -> c_int;
    FPDFPageObj_GetDashPhase: fn(FPDF_PAGEOBJECT, *mut c_float) -> FPDF_BOOL;
    FPDFPageObj_GetDashCount: fn(FPDF_PAGEOBJECT) -> c_int;
    FPDFPageObj_GetDashArray: fn(FPDF_PAGEOBJECT, *mut c_float, usize) -> FPDF_BOOL;
    FPDFPageObj_GetRenderedStrokePattern: fn(FPDF_DOCUMENT, FPDF_PAGEOBJECT) -> FPDF_BITMAP;
    FPDFPageObj_GetRenderedFillPattern: fn(FPDF_DOCUMENT, FPDF_PAGEOBJECT) -> FPDF_BITMAP;
    FPDFPageObj_GetClipPath: fn(FPDF_PAGEOBJECT) -> FPDF_CLIPPATH;
    FPDFPageObj_TransformClipPath: fn(FPDF_PAGEOBJECT, c_double, c_double, c_double, c_double, c_double, c_double);
    FPDFClipPath_CountPaths: fn(FPDF_CLIPPATH) -> c_int;
    FPDFClipPath_CountPathSegments: fn(FPDF_CLIPPATH, c_int) -> c_int;
    FPDFClipPath_GetPathSegment: fn(FPDF_CLIPPATH, c_int, c_int) -> FPDF_PATHSEGMENT;
    FPDFPath_CountSegments: fn(FPDF_PAGEOBJECT) -> c_int;
    FPDFPath_GetPathSegment: fn(FPDF_PAGEOBJECT, c_int) -> FPDF_PATHSEGMENT;
    FPDFPath_GetDrawMode: fn(FPDF_PAGEOBJECT, *mut c_int, *mut FPDF_BOOL) -> FPDF_BOOL;
    FPDFPathSegment_GetPoint: fn(FPDF_PATHSEGMENT, *mut c_float, *mut c_float) -> FPDF_BOOL;
    FPDFPathSegment_GetType: fn(FPDF_PATHSEGMENT) -> c_int;
    FPDFPathSegment_GetClose: fn(FPDF_PATHSEGMENT) -> FPDF_BOOL;
    FPDFTextObj_GetFontSize: fn(FPDF_PAGEOBJECT, *mut c_float) -> FPDF_BOOL;
    FPDFTextObj_GetTextRenderMode: fn(FPDF_PAGEOBJECT) -> c_int;
    FPDFTextObj_GetFont: fn(FPDF_PAGEOBJECT) -> FPDF_FONT;
    FPDFFont_GetFamilyName: fn(FPDF_FONT, *mut std::ffi::c_char, usize) -> usize;
    FPDFFont_GetFlags: fn(FPDF_FONT) -> c_int;
    FPDFFont_GetGlyphWidth: fn(FPDF_FONT, u32, c_float, *mut c_float) -> FPDF_BOOL;
    FPDFAnnot_HasKey: fn(FPDF_ANNOTATION, *const std::ffi::c_char) -> FPDF_BOOL;
    FPDFFont_GetWeight: fn(FPDF_FONT) -> c_int;
    FPDFFont_GetGlyphPath: fn(FPDF_FONT, u32, c_float) -> FPDF_GLYPHPATH;
    FPDFGlyphPath_CountGlyphSegments: fn(FPDF_GLYPHPATH) -> c_int;
    FPDFGlyphPath_GetGlyphPathSegment: fn(FPDF_GLYPHPATH, c_int) -> FPDF_PATHSEGMENT;
    FPDFImageObj_GetBitmap: fn(FPDF_PAGEOBJECT) -> FPDF_BITMAP;
    FPDFImageObj_GetRenderedBitmap: fn(FPDF_DOCUMENT, FPDF_PAGE, FPDF_PAGEOBJECT) -> FPDF_BITMAP;
    FPDFImageObj_GetImageDataRaw: fn(FPDF_PAGEOBJECT, *mut c_void, c_ulong) -> c_ulong;
    FPDFImageObj_GetImageFilterCount: fn(FPDF_PAGEOBJECT) -> c_int;
    FPDFImageObj_GetImageFilter: fn(FPDF_PAGEOBJECT, c_int, *mut c_void, c_ulong) -> c_ulong;
    FPDFImageObj_GetIccProfileDataDecoded: fn(FPDF_PAGEOBJECT, FPDF_PAGE, *mut u8, usize, *mut usize) -> FPDF_BOOL;
    FPDFImageObj_GetImageMetadata: fn(FPDF_PAGEOBJECT, FPDF_PAGE, *mut FPDF_IMAGEOBJ_METADATA) -> FPDF_BOOL;
    FPDFBitmap_GetFormat: fn(FPDF_BITMAP) -> c_int;
    FPDFBitmap_GetBuffer: fn(FPDF_BITMAP) -> *mut c_void;
    FPDFBitmap_GetWidth: fn(FPDF_BITMAP) -> c_int;
    FPDFBitmap_GetHeight: fn(FPDF_BITMAP) -> c_int;
    FPDFBitmap_GetStride: fn(FPDF_BITMAP) -> c_int;
    FPDFPage_GetAnnotCount: fn(FPDF_PAGE) -> c_int;
    FPDFPage_GetAnnot: fn(FPDF_PAGE, c_int) -> FPDF_ANNOTATION;
    FPDFPage_CloseAnnot: fn(FPDF_ANNOTATION);
    FPDFAnnot_GetSubtype: fn(FPDF_ANNOTATION) -> c_int;
}

/// Why PDFium couldn't be brought up. Separate from [`FailCode`] because a worker reports this once, at start.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LibraryError {
    /// The library file is missing or isn't loadable.
    LibraryMissing,
    /// A function this seam needs isn't in the library, so it's the wrong version.
    WrongLibrary,
}

/// The library, once per process, and never unloaded.
pub struct Pdfium {
    api: &'static Api,
}

static API: OnceLock<Result<Api, LibraryError>> = OnceLock::new();
/// Set while a document is open. PDFium's global state makes a second one unsafe however carefully it's used.
static DOCUMENT_OPEN: AtomicBool = AtomicBool::new(false);

impl Pdfium {
    /// Loads and initialises the library at `path`. Later calls return the same library whatever path they name.
    pub fn load(path: &Path) -> Result<Self, LibraryError> {
        let loaded = API.get_or_init(|| {
            let library = Library::open(path).map_err(|_| LibraryError::LibraryMissing)?;
            let api = Api::load(&library).ok_or(LibraryError::WrongLibrary)?;
            // SAFETY: the symbol was resolved above, and `get_or_init` means this runs once in the process.
            unsafe { (api.FPDF_InitLibrary)() };
            // `Library` has no destructor: the library stays loaded for the process's life, and `api` points into it.
            Ok(api)
        });
        match loaded {
            Ok(api) => Ok(Self { api }),
            Err(error) => Err(*error),
        }
    }
}

/// `m_GetBlock`: reads through the descriptor the host passed. A failure tells PDFium the block is unavailable; nothing crosses as a panic.
unsafe extern "C" fn get_block(param: *mut c_void, position: c_ulong, buf: *mut c_uchar, size: c_ulong) -> c_int {
    // SAFETY: `param` is the `Box<File>` this backend holds for as long as the document is open, and PDFium's contract is that `buf` has
    // `size` writable bytes.
    let (file, out) = unsafe { (&*param.cast::<File>(), std::slice::from_raw_parts_mut(buf, size as usize)) };
    // `c_ulong` is `u64` here and `u32` on Windows.
    #[allow(clippy::useless_conversion)]
    let position = u64::from(position);
    c_int::from(recto_platform::read_exact_at(file, out, position).is_ok())
}

/// The pause callback's state. PDFium hands back a pointer to `base`, the first field, so a cast finds the rest.
#[repr(C)]
struct PauseState<'a> {
    base: IFSDK_PAUSE,
    cancel: &'a dyn Cancel,
    cancelled: Cell<bool>,
}

unsafe extern "C" fn need_to_pause(this: *mut IFSDK_PAUSE) -> FPDF_BOOL {
    // SAFETY: PDFium passes back the pointer `draw` gave it, which is `base` at offset zero of a `repr(C)` `PauseState` alive for that call.
    let state = unsafe { &*this.cast::<PauseState<'_>>() };
    let cancelled = state.cancel.is_cancelled();
    state.cancelled.set(cancelled);
    FPDF_BOOL::from(cancelled)
}

/// The one open page, kept because tiles arrive in runs for the same page and `FPDF_LoadPage` is the slow part.
struct OpenPage {
    index: u32,
    page: FPDF_PAGE,
    text: FPDF_TEXTPAGE,
    /// Whether `FORM_OnAfterLoadPage` ran, so `FORM_OnBeforeClosePage` runs exactly once.
    form_loaded: bool,
}

/// One document in PDFium.
pub struct PdfiumBackend {
    api: &'static Api,
    document: FPDF_DOCUMENT,
    form: FPDF_FORMHANDLE,
    form_info: Box<FPDF_FORMFILLINFO>,
    open_page: Option<OpenPage>,
    page_count: u32,
    /// Holds `m_Param`'s `File` and the access struct for as long as the document reads them.
    _access: Box<FPDF_FILEACCESS>,
    _file: Box<File>,
    /// `!Send` and `!Sync`: PDFium isn't thread-safe.
    _one_thread: PhantomData<*mut ()>,
    /// A page's contents on their way out, a frame at a time. Goes before the page it's about, and any other job for any page ends it.
    walk: Option<Box<Walk>>,
}

impl PdfiumBackend {
    /// Opens a document on an open file. PDFium reads it through `pread` as it needs bytes, with no file path anywhere in the process.
    pub fn open(pdfium: &Pdfium, file: File, password: Option<&[u8]>) -> Result<Self, RenderError> {
        if DOCUMENT_OPEN.swap(true, Ordering::AcqRel) {
            return Err(FailCode::Internal);
        }
        let opened = Self::open_inner(pdfium, file, password);
        if opened.is_err() {
            DOCUMENT_OPEN.store(false, Ordering::Release);
        }
        opened
    }

    fn open_inner(pdfium: &Pdfium, file: File, password: Option<&[u8]>) -> Result<Self, RenderError> {
        let api = pdfium.api;
        let len = file.metadata().map_err(|_| FailCode::Unreadable)?.len();
        let len = c_ulong::try_from(len).map_err(|_| FailCode::Unreadable)?;
        let password = password.map(CString::new).transpose().map_err(|_| FailCode::NeedsPassword)?;
        let file = Box::new(file);
        let mut access = Box::new(FPDF_FILEACCESS {
            m_FileLen: len,
            m_GetBlock: get_block,
            m_Param: std::ptr::from_ref::<File>(&file).cast_mut().cast(),
        });
        let password_ptr = password.as_ref().map_or(std::ptr::null(), |p| p.as_ptr());
        // SAFETY: `access` and the file it names are boxed and held in `Self` for as long as the document is open, as PDFium requires.
        // The password only has to last for this call.
        let document = unsafe { (api.FPDF_LoadCustomDocument)(&raw mut *access, password_ptr) };
        if document.is_null() {
            // SAFETY: no arguments, and this thread just made the failing call.
            return Err(match unsafe { (api.FPDF_GetLastError)() } {
                FPDF_ERR_PASSWORD => FailCode::NeedsPassword,
                FPDF_ERR_SECURITY => FailCode::Security,
                FPDF_ERR_FILE => FailCode::Unreadable,
                FPDF_ERR_FORMAT => FailCode::NotPdf,
                _ => FailCode::Internal,
            });
        }
        // SAFETY: `document` was just checked to be live.
        let page_count = u32::try_from(unsafe { (api.FPDF_GetPageCount)(document) }).unwrap_or(0).min(MAX_PAGE_COUNT);
        Ok(Self {
            api,
            document,
            form: std::ptr::null_mut(),
            form_info: Box::new(FPDF_FORMFILLINFO { version: 2, callbacks: [std::ptr::null_mut(); 64] }),
            open_page: None,
            page_count,
            _access: access,
            _file: file,
            _one_thread: PhantomData,
            walk: None,
        })
    }

    /// The form environment, made on first draw so a text job doesn't pay for it. It draws widgets; `FPDF_ANNOT` skips them.
    fn ensure_form(&mut self) {
        if self.form.is_null() {
            // SAFETY: the document is live, and `form_info` is boxed in `Self`, outliving the environment, as PDFium requires.
            self.form = unsafe { (self.api.FPDFDOC_InitFormFillEnvironment)(self.document, &raw mut *self.form_info) };
        }
    }

    /// Makes `index` the open page, closing any other.
    fn load_page(&mut self, index: u32) -> Result<(), RenderError> {
        // A walk turns objects off and changes their opacity, so nothing else may see the page it was on.
        if self.walk.is_some() {
            self.close_page();
        }
        self.load_page_for_walk(index)
    }

    /// Makes `index` the open page, for a walk that manages what it's done to it.
    fn load_page_for_walk(&mut self, index: u32) -> Result<(), RenderError> {
        if index >= self.page_count {
            return Err(FailCode::BadPage);
        }
        if self.open_page.as_ref().is_some_and(|open| open.index == index) {
            return Ok(());
        }
        self.close_page();
        // SAFETY: the document is live and `index` is in range, so it fits `c_int` (the count came from one).
        let page = unsafe { (self.api.FPDF_LoadPage)(self.document, index as c_int) };
        if page.is_null() {
            return Err(FailCode::BadPage);
        }
        self.open_page = Some(OpenPage { index, page, text: std::ptr::null_mut(), form_loaded: false });
        Ok(())
    }

    fn close_page(&mut self) {
        self.walk = None;
        if let Some(open) = self.open_page.take() {
            // SAFETY: these handles came from this document, are live, and `take` means nothing can reach them after this. The text page
            // and form view must go before the page they refer to.
            unsafe {
                if !open.text.is_null() {
                    (self.api.FPDFText_ClosePage)(open.text);
                }
                if open.form_loaded {
                    (self.api.FORM_OnBeforeClosePage)(open.page, self.form);
                }
                (self.api.FPDF_ClosePage)(open.page);
            }
        }
    }

    /// The page that `load_page` left open.
    fn open_page_handle(&self) -> Option<FPDF_PAGE> {
        self.open_page.as_ref().map(|open| open.page)
    }

    /// Loads the page and draws `rect` of it at `scale` into `out`. False if cancelled.
    fn draw_rect(&mut self, page: u32, rect: TileRect, scale: f32, out: &mut [u8], stride: u32, cancel: &dyn Cancel) -> Result<bool, RenderError> {
        if cancel.is_cancelled() {
            return Ok(false);
        }
        let size = self.page_size(page)?;
        let (full_width, full_height) = device_size(size.width, size.height, scale).map_err(|_| FailCode::TooLarge)?;
        if rect.x >= full_width || rect.y >= full_height {
            return Err(FailCode::BadRequest);
        }
        let int = |value: u32| c_int::try_from(value).map_err(|_| FailCode::TooLarge);
        let (width, height, stride_int) = (int(rect.width)?, int(rect.height)?, int(stride)?);
        let at = (-int(rect.x)?, -int(rect.y)?);
        let full = (int(full_width)?, int(full_height)?);
        self.load_page(page)?;
        // Page load can't be interrupted, so this is the first chance to notice it took too long.
        if cancel.is_cancelled() {
            return Ok(false);
        }
        self.draw(out, width, height, stride_int, at, full, cancel)
    }

    /// Readies the open page's form fields for drawing, if the environment can be made.
    fn ready_form_view(&mut self) {
        self.ensure_form();
        let (api, form) = (self.api, self.form);
        if let Some(open) = self.open_page.as_mut().filter(|open| !open.form_loaded && !form.is_null()) {
            // SAFETY: the page is live and `form` came from this document. Once per open page, as `form_loaded` says.
            unsafe { (api.FORM_OnAfterLoadPage)(open.page, form) };
            open.form_loaded = true;
        }
    }

    /// The open page's text page, made if need be. It goes with the page.
    fn text_page_now(&mut self) -> Result<FPDF_TEXTPAGE, RenderError> {
        let api = self.api;
        let open = self.open_page.as_mut().ok_or(FailCode::Internal)?;
        if open.text.is_null() {
            // SAFETY: the page is live, and the text page is closed with it in `close_page`.
            open.text = unsafe { (api.FPDFText_LoadPage)(open.page) };
        }
        if open.text.is_null() { Err(FailCode::Internal) } else { Ok(open.text) }
    }

    /// Draws into `out` at the given whole-page size and offset. Returns false if cancelled. Page load has no pause, so a slow one only
    /// notices cancelling here.
    fn draw(&mut self, out: &mut [u8], width: c_int, height: c_int, stride: c_int, at: (c_int, c_int), full: (c_int, c_int), cancel: &dyn Cancel) -> Result<bool, RenderError> {
        let needed = i64::from(stride) * i64::from(height);
        if needed > out.len() as i64 || stride < width.saturating_mul(4) {
            return Err(FailCode::BadSurface);
        }
        self.ensure_form();
        let Some(page) = self.open_page_handle() else { return Err(FailCode::Internal) };
        let api = self.api;
        // SAFETY: `page` is live and `form` came from this document. `FORM_OnAfterLoadPage` runs once per open page.
        if let Some(open) = self.open_page.as_mut().filter(|open| !open.form_loaded && !self.form.is_null()) {
            unsafe { (api.FORM_OnAfterLoadPage)(page, self.form) };
            open.form_loaded = true;
        }
        // SAFETY: `out` holds at least `stride * height` bytes, checked above, and outlives the bitmap, destroyed below in this function.
        let bitmap = unsafe { (api.FPDFBitmap_CreateEx)(width, height, FPDFBitmap_BGRx, out.as_mut_ptr().cast(), stride) };
        if bitmap.is_null() {
            return Err(FailCode::Internal);
        }
        let mut pause = PauseState {
            base: IFSDK_PAUSE { version: 1, NeedToPauseNow: need_to_pause, user: std::ptr::null_mut() },
            cancel,
            cancelled: Cell::new(false),
        };
        let pause_ptr = (&raw mut pause).cast::<IFSDK_PAUSE>();
        // SAFETY: `bitmap` and `page` are live. `pause` is alive across every call that receives it, and its first field is the struct
        // PDFium expects. `FPDF_RenderPage_Close` ends the progressive state even if the loop stops early, before `pause` is dropped.
        let done = unsafe {
            (api.FPDFBitmap_FillRect)(bitmap, 0, 0, width, height, PAPER_WHITE);
            let mut status = (api.FPDF_RenderPageBitmap_Start)(bitmap, page, at.0, at.1, full.0, full.1, 0, FPDF_ANNOT, pause_ptr);
            while status == FPDF_RENDER_TOBECONTINUED && !cancel.is_cancelled() {
                status = (api.FPDF_RenderPage_Continue)(page, pause_ptr);
            }
            let done = status == FPDF_RENDER_DONE;
            if done && !self.form.is_null() {
                (api.FPDF_FFLDraw)(self.form, bitmap, page, at.0, at.1, full.0, full.1, 0, FPDF_ANNOT);
            }
            (api.FPDF_RenderPage_Close)(page);
            (api.FPDFBitmap_Destroy)(bitmap);
            done
        };
        if done || pause.cancelled.get() || cancel.is_cancelled() {
            Ok(done)
        } else {
            // Neither finished nor cancelled means `FPDF_RENDER_FAILED`.
            Err(FailCode::Internal)
        }
    }
}

impl RenderBackend for PdfiumBackend {
    fn page_count(&self) -> u32 {
        self.page_count
    }

    fn page_size(&mut self, page: u32) -> Result<PagePoints, RenderError> {
        if page >= self.page_count {
            return Err(FailCode::BadPage);
        }
        let (mut width, mut height) = (0.0, 0.0);
        // SAFETY: the document is live, the index is in range, and the out-pointers are live for the call.
        let ok = unsafe { (self.api.FPDF_GetPageSizeByIndex)(self.document, page as c_int, &raw mut width, &raw mut height) };
        if ok == 0 {
            return Err(FailCode::BadPage);
        }
        Ok(PagePoints { width: width as c_float, height: height as c_float })
    }

    fn render_tile(&mut self, page: u32, rect: TileRect, scale: f32, out: &mut [u8], stride: u32, cancel: &dyn Cancel) -> Result<TileOutcome, RenderError> {
        if self.draw_rect(page, rect, scale, out, stride, cancel)? {
            Ok(TileOutcome::Done(classify_bgrx(out, rect.width, rect.height, stride)))
        } else {
            Ok(TileOutcome::Cancelled)
        }
    }

    fn render_region(
        &mut self,
        page: u32,
        rect: TileRect,
        scale: f32,
        tile_edge: u32,
        out: &mut [u8],
        stride: u32,
        cancel: &dyn Cancel,
        kinds: &mut Vec<TileKind>,
    ) -> Result<TileOutcome, RenderError> {
        if !self.draw_rect(page, rect, scale, out, stride, cancel)? {
            return Ok(TileOutcome::Cancelled);
        }
        let mut whole = None;
        for tile in region_tiles(rect, tile_edge) {
            let offset = (tile.y - rect.y) as usize * stride as usize + (tile.x - rect.x) as usize * 4;
            let pixels = out.get(offset..).ok_or(FailCode::BadSurface)?;
            let kind = classify_bgrx(pixels, tile.width, tile.height, stride);
            kinds.push(kind);
            whole = Some(whole.map_or(kind, |so_far| merge_kinds(so_far, kind)));
        }
        Ok(TileOutcome::Done(whole.unwrap_or(TileKind::Colour)))
    }

    fn contents_frame(&mut self, job: &ContentsJob, out: &mut Vec<u8>, cancel: &dyn Cancel) -> Result<(), RenderError> {
        PdfiumBackend::contents_frame(self, job, out, cancel)
    }

    fn largest_image_pixels(&mut self, page: u32) -> Result<u64, RenderError> {
        self.load_page(page)?;
        let Some(handle) = self.open_page_handle() else { return Err(FailCode::Internal) };
        let api = self.api;
        let mut largest = 0u64;
        let mut looked_at = 0usize;
        // SAFETY: the page is live for this whole function, and every object comes from it or a form on it. Indexes are below the counts just read.
        unsafe {
            let mut waiting: Vec<(FPDF_PAGEOBJECT, u32)> = Vec::new();
            for index in 0..(api.FPDFPage_CountObjects)(handle).max(0).min(MAX_OBJECTS_LOOKED_AT as c_int) {
                waiting.push(((api.FPDFPage_GetObject)(handle, index), 0));
            }
            while let Some((object, depth)) = waiting.pop() {
                looked_at += 1;
                if object.is_null() || looked_at > MAX_OBJECTS_LOOKED_AT {
                    continue;
                }
                match (api.FPDFPageObj_GetType)(object) {
                    FPDF_PAGEOBJ_IMAGE => {
                        let (mut width, mut height): (c_uint, c_uint) = (0, 0);
                        if (api.FPDFImageObj_GetImagePixelSize)(object, &raw mut width, &raw mut height) != 0 {
                            largest = largest.max(u64::from(width) * u64::from(height));
                        }
                    }
                    FPDF_PAGEOBJ_FORM if depth < MAX_FORM_DEPTH => {
                        let room = MAX_OBJECTS_LOOKED_AT.saturating_sub(waiting.len());
                        for index in 0..((api.FPDFFormObj_CountObjects)(object).max(0) as usize).min(room) {
                            waiting.push(((api.FPDFFormObj_GetObject)(object, index as c_ulong), depth + 1));
                        }
                    }
                    _ => {}
                }
            }
        }
        Ok(largest)
    }

    fn image_boxes(&mut self, page: u32) -> Result<Vec<[u16; 4]>, RenderError> {
        self.load_page(page)?;
        let Some(handle) = self.open_page_handle() else { return Err(FailCode::Internal) };
        let api = self.api;
        let mut boxes: Vec<[u16; 4]> = Vec::new();
        let mut looked_at = 0usize;
        // Where a point of the page lands, as a part of the page. By PDFium's own mapping, so a rotated page and a cropped one come out right, however they got that way.
        const PARTS: c_int = 65_535;
        let place = |x: f32, y: f32| -> Option<(u16, u16)> {
            let (mut device_x, mut device_y): (c_int, c_int) = (0, 0);
            // SAFETY: the page is live for this whole function, and the two out-pointers are valid for writes.
            let ok = unsafe { (api.FPDF_PageToDevice)(handle, 0, 0, PARTS, PARTS, 0, f64::from(x), f64::from(y), &raw mut device_x, &raw mut device_y) };
            (ok != 0).then(|| (device_x.clamp(0, PARTS) as u16, device_y.clamp(0, PARTS) as u16))
        };
        // SAFETY: the page is live for this whole function, and every object comes from it or a form on it. Indexes are below the counts just read.
        unsafe {
            let mut waiting: Vec<(FPDF_PAGEOBJECT, u32, FPDF_PAGEOBJECT)> = Vec::new();
            for index in 0..(api.FPDFPage_CountObjects)(handle).max(0).min(MAX_OBJECTS_LOOKED_AT as c_int) {
                let object = (api.FPDFPage_GetObject)(handle, index);
                waiting.push((object, 0, object));
            }
            while let Some((object, depth, top)) = waiting.pop() {
                looked_at += 1;
                if object.is_null() || looked_at > MAX_OBJECTS_LOOKED_AT {
                    continue;
                }
                match (api.FPDFPageObj_GetType)(object) {
                    // An image in a form has its bounds in the form's own space, so it's the outermost form's that say where it is on the page.
                    FPDF_PAGEOBJ_IMAGE => {
                        let (mut left, mut bottom, mut right, mut top_edge) = (0.0, 0.0, 0.0, 0.0);
                        if (api.FPDFPageObj_GetBounds)(top, &raw mut left, &raw mut bottom, &raw mut right, &raw mut top_edge) != 0
                            && let (Some(one), Some(two)) = (place(left, top_edge), place(right, bottom))
                        {
                            let found = [one.0.min(two.0), one.1.min(two.1), one.0.max(two.0), one.1.max(two.1)];
                            if !boxes.contains(&found) {
                                boxes.push(found);
                            }
                        }
                    }
                    FPDF_PAGEOBJ_FORM if depth < MAX_FORM_DEPTH => {
                        let room = MAX_OBJECTS_LOOKED_AT.saturating_sub(waiting.len());
                        for index in 0..((api.FPDFFormObj_CountObjects)(object).max(0) as usize).min(room) {
                            waiting.push(((api.FPDFFormObj_GetObject)(object, index as c_ulong), depth + 1, top));
                        }
                    }
                    _ => {}
                }
            }
        }
        if boxes.len() > recto_proto::MAX_IMAGE_BOXES {
            let all = boxes.iter().fold([u16::MAX, u16::MAX, 0, 0], |all, found| [all[0].min(found[0]), all[1].min(found[1]), all[2].max(found[2]), all[3].max(found[3])]);
            boxes = vec![all];
        }
        Ok(boxes)
    }

    fn page_text(&mut self, page: u32, options: TextOptions) -> Result<PageChars, RenderError> {
        self.load_page(page)?;
        let api = self.api;
        let open = self.open_page.as_mut().ok_or(FailCode::Internal)?;
        if open.text.is_null() {
            // SAFETY: the page is live, and the text page is closed with it in `close_page`.
            open.text = unsafe { (api.FPDFText_LoadPage)(open.page) };
        }
        let text = open.text;
        if text.is_null() {
            return Err(FailCode::Internal);
        }
        // SAFETY: `text` was just checked to be live.
        let count = unsafe { (api.FPDFText_CountChars)(text) }.max(0);
        if count as u32 > MAX_PAGE_CHARS {
            return Err(FailCode::TooLarge);
        }
        let want_boxes = options.boxes || options.hidden_check;
        let mut chars = Vec::with_capacity(count as usize);
        let mut boxes: Vec<CharBox> = Vec::with_capacity(if want_boxes { count as usize } else { 0 });
        let mut index = 0;
        while index < count {
            // SAFETY: `text` is live and `index` is below its character count.
            let (mut code, hyphen, generated) = unsafe {
                ((api.FPDFText_GetUnicode)(text, index), (api.FPDFText_IsHyphen)(text, index), (api.FPDFText_IsGenerated)(text, index))
            };
            // A character past the 16-bit range comes from a font's ToUnicode as two halves, each its own "character" here. Put back together, or an emoji is lost and a tag character, the plainest
            // hidden instruction there is, is never seen as one.
            let mut taken = 1;
            if (0xD800..0xDC00).contains(&code) && index + 1 < count {
                // SAFETY: as above, and `index + 1` was just checked.
                let low = unsafe { (api.FPDFText_GetUnicode)(text, index + 1) };
                if (0xDC00..0xE000).contains(&low) {
                    code = 0x1_0000 + ((code - 0xD800) << 10) + (low - 0xDC00);
                    taken = 2;
                }
            }
            let mut flags = 0;
            if hyphen == 1 {
                flags |= CharRecord::HYPHEN;
            }
            if generated == 1 {
                flags |= CharRecord::GENERATED;
            }
            chars.push(CharRecord::new(code, flags));
            if want_boxes {
                let (mut left, mut right, mut bottom, mut top) = (0.0, 0.0, 0.0, 0.0);
                // SAFETY: as above, and the out-pointers are live for the call. On failure they stay zero: an empty box, which has no ink.
                unsafe { (api.FPDFText_GetCharBox)(text, index, &raw mut left, &raw mut right, &raw mut bottom, &raw mut top) };
                let edge = |value: f64| if value.is_finite() { value as f32 } else { 0.0 };
                boxes.push([edge(left), edge(bottom), edge(right), edge(top)]);
            }
            index += taken;
        }
        // A page with no characters has nothing to hide from a text reader, and needs no drawing to say so.
        // If the page can't be drawn for the check, the text still goes back, saying it wasn't checked: nothing is marked until the drawing has worked.
        let hidden_checked = options.hidden_check && (chars.is_empty() || self.mark_no_ink(page, &mut chars, &boxes).is_ok());
        if !options.boxes {
            boxes = Vec::new();
        }
        let frame = if options.boxes { self.page_frame() } else { PageFrame::default() };
        // SAFETY: the document is live. A file with no encryption answers all ones.
        let permissions = unsafe { (api.FPDF_GetDocPermissions)(self.document) };
        Ok(PageChars { chars, boxes, hidden_checked, frame, copy_restricted: permissions & PERMISSION_COPY == 0 })
    }
}

impl PdfiumBackend {
    /// The area of the open page that's shown, and how far it's turned. All zero if PDFium can't say, which leaves the page with nowhere to select.
    fn page_frame(&self) -> PageFrame {
        let Some(open) = self.open_page.as_ref() else { return PageFrame::default() };
        let mut area = FS_RECTF { left: 0.0, top: 0.0, right: 0.0, bottom: 0.0 };
        // SAFETY: the page is live, and the out-pointer is live for the call.
        let known = unsafe { (self.api.FPDF_GetPageBoundingBox)(open.page, &raw mut area) };
        if known == 0 {
            return PageFrame::default();
        }
        // SAFETY: the page is live.
        let turns = unsafe { (self.api.FPDFPage_GetRotation)(open.page) };
        PageFrame { turns: (turns & 3) as u8, left: area.left.min(area.right), bottom: area.bottom.min(area.top), right: area.left.max(area.right), top: area.bottom.max(area.top) }
    }
}

/// The hidden-text check draws at this scale, as the prototype did, or lower for a page so large its bitmap would pass [`INK_MAX_BYTES`].
const INK_SCALE: f32 = 2.0;
const INK_MAX_BYTES: f32 = 64.0 * 1024.0 * 1024.0;

impl PdfiumBackend {
    /// Draws the page and sets `NO_INK` on each visible-looking character whose box has no ink. Nothing is removed. From the spike's
    /// prototype, §10.4. Whitespace, control characters and characters PDFium generated have no ink to find, so they're skipped.
    fn mark_no_ink(&mut self, page: u32, chars: &mut [CharRecord], boxes: &[CharBox]) -> Result<(), RenderError> {
        let size = self.page_size(page)?;
        let area_at_ink_scale = size.width * INK_SCALE * size.height * INK_SCALE * 4.0;
        let scale = if area_at_ink_scale > INK_MAX_BYTES { INK_SCALE * (INK_MAX_BYTES / area_at_ink_scale).sqrt() } else { INK_SCALE };
        let (width, height) = device_size(size.width, size.height, scale).map_err(|_| FailCode::TooLarge)?;
        let (Ok(width), Ok(height)) = (c_int::try_from(width), c_int::try_from(height)) else { return Err(FailCode::TooLarge) };
        let stride = width.checked_mul(4).ok_or(FailCode::TooLarge)?;
        let bytes = usize::try_from(i64::from(stride) * i64::from(height)).map_err(|_| FailCode::TooLarge)?;
        let mut pixels = vec![0u8; bytes];
        self.draw(&mut pixels, width, height, stride, (0, 0), (width, height), &crate::NeverCancel)?;
        let Some(page_handle) = self.open_page_handle() else { return Err(FailCode::Internal) };
        let api = self.api;
        let to_device = |x: f32, y: f32| {
            let (mut device_x, mut device_y) = (0, 0);
            // SAFETY: the page is live and the out-pointers are live for the call.
            unsafe { (api.FPDF_PageToDevice)(page_handle, 0, 0, width, height, 0, f64::from(x), f64::from(y), &raw mut device_x, &raw mut device_y) };
            (device_x, device_y)
        };
        for (record, char_box) in chars.iter_mut().zip(boxes) {
            let ch = record.ch();
            if ch.is_whitespace() || ch.is_control() || record.has(CharRecord::GENERATED) || record.has(CharRecord::HYPHEN) {
                continue;
            }
            let [left, bottom, right, top] = *char_box;
            let (x0, y0) = to_device(left, top);
            let (x1, y1) = to_device(right, bottom);
            if !crate::box_has_ink(&pixels, width, height, stride as usize, (x0, y0, x1.saturating_add(1), y1.saturating_add(1))) {
                *record = CharRecord::new(ch as u32, CharRecord::NO_INK);
            }
        }
        Ok(())
    }
}

/// Draws one page onto a caller's memory, in one go: the host's watch holds it from outside, and nothing here pauses. `form` draws form fields too, when `annots` is set.
#[allow(clippy::too_many_arguments)]
fn draw_plain(api: &'static Api, page: FPDF_PAGE, form: FPDF_FORMHANDLE, out: &mut [u8], width: c_int, height: c_int, stride: c_int, at: (c_int, c_int), full: (c_int, c_int), format: c_int, paper: c_ulong, annots: bool) -> bool {
    let needed = i64::from(stride) * i64::from(height);
    if needed > out.len() as i64 || stride < width.saturating_mul(4) {
        return false;
    }
    let flags = if annots { FPDF_ANNOT } else { 0 };
    // SAFETY: `out` holds at least `stride * height` bytes, checked above, and outlives the bitmap, destroyed below in this function. `page` is live for the caller's walk.
    unsafe {
        let bitmap = (api.FPDFBitmap_CreateEx)(width, height, format, out.as_mut_ptr().cast(), stride);
        if bitmap.is_null() {
            return false;
        }
        (api.FPDFBitmap_FillRect)(bitmap, 0, 0, width, height, paper);
        let mut pause = PauseState { base: IFSDK_PAUSE { version: 1, NeedToPauseNow: need_to_pause, user: std::ptr::null_mut() }, cancel: &crate::NeverCancel, cancelled: Cell::new(false) };
        let pause_ptr = (&raw mut pause).cast::<IFSDK_PAUSE>();
        let mut status = (api.FPDF_RenderPageBitmap_Start)(bitmap, page, at.0, at.1, full.0, full.1, 0, flags, pause_ptr);
        while status == FPDF_RENDER_TOBECONTINUED {
            status = (api.FPDF_RenderPage_Continue)(page, pause_ptr);
        }
        let done = status == FPDF_RENDER_DONE;
        if done && annots && !form.is_null() {
            (api.FPDF_FFLDraw)(form, bitmap, page, at.0, at.1, full.0, full.1, 0, flags);
        }
        (api.FPDF_RenderPage_Close)(page);
        (api.FPDFBitmap_Destroy)(bitmap);
        done
    }
}

impl Drop for PdfiumBackend {
    fn drop(&mut self) {
        self.close_page();
        // SAFETY: these handles are live and nothing can reach them after `self` goes. The environment must close before the document.
        unsafe {
            if !self.form.is_null() {
                (self.api.FPDFDOC_ExitFormFillEnvironment)(self.form);
            }
            (self.api.FPDF_CloseDocument)(self.document);
        }
        DOCUMENT_OPEN.store(false, Ordering::Release);
    }
}
