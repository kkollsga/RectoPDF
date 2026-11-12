//! Measuring what things cost: `budget`, `budget render` and `budget viewport`.

#![allow(unused_imports)]
use super::{Cmd, Cx, row_access};
use crate::human::*;
use crate::table::*;
use crate::*;
use serde::Deserialize;
use serde_json::{Value, json};
use std::fmt::Write;
use std::path::PathBuf;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Budget {}

impl Cmd for Budget {
    const TAG: &'static str = "budget";
    const ROWS: &'static [Spec] = ROWS_BUDGET;
    const DRAWS: bool = false;
    const READS_PAGES: bool = false;
    const PROTECTED_OK: bool = false;
    fn tab(&self) -> Option<&str> {
        None
    }
    fn access(&self) -> Access {
        row_access(Self::TAG)
    }
    fn route(&self) -> Runs {
        Runs::Session
    }
    fn log_details(&self) -> Value {
        Value::Null
    }
    fn run<A: SessionAccess>(&self, cx: &mut Cx<'_, A>) -> Result<Value, AgentError> {
        let access = cx.access;
        Ok(json!({
            "tabs": access.with(|s| s.tabs().len()),
            "footprint_bytes": recto_platform::footprint_bytes(),
            "open_handles": recto_platform::open_handle_count(),
        }))
    }
    fn human(data: &Value) -> String {
        format!(
            "{}, footprint {}, {}",
            count(number(&data["tabs"]), "tab", "tabs"),
            bytes(number(&data["footprint_bytes"])),
            count(number(&data["open_handles"]), "handle", "handles")
        )
    }
}

/// What a worker costs: idle, per page, peak, and how often it was retired.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BudgetRender {
    #[serde(default = "crate::active_tab")]
    pub tab: String,
    pub pages: u32,
    /// One-based first page. The first page of the file if left out.
    #[serde(default)]
    pub first: Option<u32>,
    #[serde(default = "crate::render::default_scale")]
    pub scale: f32,
    /// A fresh worker for every page, to see what one page costs by itself.
    #[serde(default)]
    pub fresh: bool,
    /// 512 px tiles across each page, rather than whole pages.
    #[serde(default)]
    pub tiles: bool,
    /// Each page's numbers, as well as the summary.
    #[serde(default)]
    pub samples: bool,
    /// Overrides the hard memory limit for this measurement, up to 4 GiB.
    #[serde(default)]
    pub limit_mib: Option<u32>,
    /// Overrides the retirement footprint for this measurement.
    #[serde(default)]
    pub retire_mib: Option<u32>,
}

impl Cmd for BudgetRender {
    const TAG: &'static str = "budget_render";
    const ROWS: &'static [Spec] = ROWS_BUDGET_RENDER;
    const DRAWS: bool = true;
    const READS_PAGES: bool = true;
    const PROTECTED_OK: bool = false;
    fn tab(&self) -> Option<&str> {
        Some(&self.tab)
    }
    fn access(&self) -> Access {
        // Measuring with the decided limits only reads numbers. Choosing other limits decides what a hostile file may use, so it isn't reading.
        if self.limit_mib.is_some() || self.retire_mib.is_some() {
            return Access::At(Level::Full);
        }
        row_access(Self::TAG)
    }
    fn route(&self) -> Runs {
        Runs::Session
    }
    fn log_details(&self) -> Value {
        let Self { tab, .. } = self;
        json!({ "tab": super::log_tab(tab) })
    }
    fn run<A: SessionAccess>(&self, cx: &mut Cx<'_, A>) -> Result<Value, AgentError> {
        let Self {
            tab,
            pages,
            first,
            scale,
            fresh,
            tiles,
            samples,
            limit_mib,
            retire_mib,
        } = self;
        let access = cx.access;
        {
            let path = access.with(|s| tab_path(s, select(s, tab)?))?;
            let args = BudgetArgs {
                pages: *pages,
                first: *first,
                scale: *scale,
                fresh: *fresh,
                tiles: *tiles,
                samples: *samples,
                limit_mib: *limit_mib,
                retire_mib: *retire_mib,
            };
            render::budget(&path, &args)
        }
    }
    fn human(data: &Value) -> String {
        pretty(data)
    }
}

/// What a reader's window costs: first view, one scroll step, one zoom step, with peak footprint.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BudgetViewport {
    #[serde(default = "crate::active_tab")]
    pub tab: String,
    /// `WIDTHxHEIGHT` in device pixels, such as `3024x1964`.
    pub viewport: String,
    /// One-based first page. The first page of the file if left out.
    #[serde(default)]
    pub first: Option<u32>,
    #[serde(default = "crate::render::default_scale")]
    pub scale: f32,
    /// `per_tile` or `region`, for comparing with the default, which picks by page.
    #[serde(default)]
    pub strategy: Option<String>,
}

impl Cmd for BudgetViewport {
    const TAG: &'static str = "budget_viewport";
    const ROWS: &'static [Spec] = ROWS_BUDGET_VIEWPORT;
    const DRAWS: bool = true;
    const READS_PAGES: bool = true;
    const PROTECTED_OK: bool = false;
    fn tab(&self) -> Option<&str> {
        Some(&self.tab)
    }
    fn access(&self) -> Access {
        row_access(Self::TAG)
    }
    fn route(&self) -> Runs {
        Runs::Session
    }
    fn log_details(&self) -> Value {
        let Self { tab, .. } = self;
        json!({ "tab": super::log_tab(tab) })
    }
    fn run<A: SessionAccess>(&self, cx: &mut Cx<'_, A>) -> Result<Value, AgentError> {
        let Self {
            tab,
            viewport,
            first,
            scale,
            strategy,
        } = self;
        let access = cx.access;
        {
            let path = access.with(|s| tab_path(s, select(s, tab)?))?;
            render::budget_viewport(&path, viewport, *first, *scale, strategy.as_deref())
        }
    }
    fn human(data: &Value) -> String {
        pretty(data)
    }
}

pub(crate) const ROWS_BUDGET: &[Spec] = &[Spec {
    name: "budget",
    tag: "budget",
    group: GROUP_MEASURE,
    summary: "The session's own footprint, tabs and handles right now.",
    data: "`{ tabs, footprint_bytes, open_handles }`.",
    examples: &["recto budget --json"],
    ..BASE
}];

pub(crate) const ROWS_BUDGET_RENDER: &[Spec] = &[Spec {
    name: "budget render",
    tag: "budget_render",
    group: GROUP_MEASURE,
    summary: "Measures a worker on a tab's document: idle, pages, peak and retirement. Choosing limits needs Full.",
    args: &[
        Arg {
            key: "pages",
            flag: Some("--count"),
            kind: Kind::Count(1_000_000),
            required: true,
            help: "How many pages.",
        },
        FIRST,
        SCALE,
        Arg {
            key: "fresh",
            flag: Some("--fresh"),
            kind: Kind::Switch,
            required: false,
            help: "A fresh worker per page.",
        },
        Arg {
            key: "tiles",
            flag: Some("--tiles"),
            kind: Kind::Switch,
            required: false,
            help: "512 px tiles, not whole pages.",
        },
        Arg {
            key: "samples",
            flag: Some("--samples"),
            kind: Kind::Switch,
            required: false,
            help: "Each page's numbers too.",
        },
        Arg {
            key: "limit_mib",
            flag: Some("--limit-mib"),
            kind: Kind::Count(1_000_000),
            required: false,
            help: "The memory limit, in MiB. Needs Full.",
        },
        Arg {
            key: "retire_mib",
            flag: Some("--retire-mib"),
            kind: Kind::Count(1_000_000),
            required: false,
            help: "The retirement footprint, in MiB. Needs Full.",
        },
        TAB,
    ],
    data: "Footprints in `*_bytes`, kills, tiles, and `ms: { p50, p95, max }`. `walled` says every worker it started was sandboxed with its Mach rights dropped, as the app's helper is, and a bare development worker isn't.",
    examples: &[
        "recto budget render --count 200 --samples",
        "recto budget render --count 5 --limit-mib 512 --tab \"#1\"",
    ],
    client_wait_s: 1800,
    ..BASE
}];

pub(crate) const ROWS_BUDGET_VIEWPORT: &[Spec] = &[Spec {
    name: "budget viewport",
    tag: "budget_viewport",
    group: GROUP_MEASURE,
    summary: "Measures a reader's window: first view, one scroll step, one zoom step.",
    args: &[
        Arg {
            key: "viewport",
            flag: Some("--viewport"),
            kind: Kind::Size,
            required: true,
            help: "`WIDTHxHEIGHT` in device pixels, such as `3024x1964`.",
        },
        FIRST,
        SCALE,
        Arg {
            key: "strategy",
            flag: Some("--strategy"),
            kind: Kind::Choice(&["per_tile", "region"]),
            required: false,
            help: "`per_tile` or `region`. Picked by page if left out.",
        },
        TAB,
    ],
    data: "`{ strategy, largest_image_pixels, viewport, scale, steps: [ { step, ms, tiles, passes, blank, grey, colour, low_res, failed, peak_footprint_bytes } ], spawns, retirements, host_footprint_bytes }`.",
    examples: &[
        "recto budget viewport --viewport 3024x1964",
        "recto budget viewport --viewport 1600x1000 --strategy region --first 2",
    ],
    client_wait_s: 600,
    ..BASE
}];
