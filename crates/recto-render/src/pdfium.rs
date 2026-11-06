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
//! **Not thread-safe.** PDFium holds global state. So there's one library per process, one live document ([`PdfiumBackend::open`]
//! refuses a second), and the backend is `!Send`. Parallelism comes from more worker processes, which measured better anyway (spike §4).

#![allow(unsafe_code, non_snake_case, non_camel_case_types, clippy::too_many_arguments)]

use crate::{Cancel, PageChars, PagePoints, RenderBackend, RenderError, TextOptions, TileOutcome, classify_bgrx};
use recto_platform::isolate::Library;
use recto_proto::{CharBox, CharRecord, FailCode, MAX_PAGE_CHARS, MAX_PAGE_COUNT, TileKind, TileRect, device_size, merge_kinds, region_tiles};
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
type FPDF_BOOL = c_int;

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
    FPDFText_LoadPage: fn(FPDF_PAGE) -> FPDF_TEXTPAGE;
    FPDFText_ClosePage: fn(FPDF_TEXTPAGE);
    FPDFText_CountChars: fn(FPDF_TEXTPAGE) -> c_int;
    FPDFText_GetUnicode: fn(FPDF_TEXTPAGE, c_int) -> c_uint;
    FPDFText_IsHyphen: fn(FPDF_TEXTPAGE, c_int) -> c_int;
    FPDFText_IsGenerated: fn(FPDF_TEXTPAGE, c_int) -> c_int;
    FPDFText_GetCharBox: fn(FPDF_TEXTPAGE, c_int, *mut c_double, *mut c_double, *mut c_double, *mut c_double) -> FPDF_BOOL;
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
        for index in 0..count {
            // SAFETY: `text` is live and `index` is below its character count.
            let (code, hyphen, generated) = unsafe {
                ((api.FPDFText_GetUnicode)(text, index), (api.FPDFText_IsHyphen)(text, index), (api.FPDFText_IsGenerated)(text, index))
            };
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
        }
        // A page with no characters has nothing to hide from a text reader, and needs no drawing to say so.
        // If the page can't be drawn for the check, the text still goes back, saying it wasn't checked: nothing is marked until the drawing has worked.
        let hidden_checked = options.hidden_check && (chars.is_empty() || self.mark_no_ink(page, &mut chars, &boxes).is_ok());
        if !options.boxes {
            boxes = Vec::new();
        }
        Ok(PageChars { chars, boxes, hidden_checked })
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
