//! **The sidebar's Find: its records, the person's controls, and what a command asks of it** (step R2.3, C5g). Typing in the field, ⌘F, Escape, ⌘G and
//! ⇧⌘G, Return and ⇧Return and a click on a hit's row are `find <query>`, `find --open`, `find --close`, `find --next|--previous [--jump]` and
//! `find --hit N`, the command an agent sends, run for the person by the one executor. What it shows and when the host is told are `crate::find`'s; this
//! hands it the app's parts it reaches, as a [`FindHandle`], and converts what comes back.

use super::{Inner, lock, person};
use crate::find::{self as app, FindHandle};
pub use crate::find::{FIELD_HEIGHT, FindCall, Heights, ROW_HEIGHT, heights};
pub use crate::find::{FindRow, FindRowKind, FindView};
use recto_agent::Code;
use recto_agent::commands::Move;
use recto_agent::commands::finding::{Find, FindAsk};
use recto_agent::{Command, Effect, Event};
use serde_json::Value;
use std::sync::Arc;

/// What Find reaches of the app, for one call.
pub(super) fn handle<R>(inner: &Inner, step: impl FnOnce(&FindHandle<'_, dyn crate::Listener>) -> R) -> R {
    step(&FindHandle {
        session: &inner.host.session,
        field: &inner.tools.find_field,
        frozen: &inner.core.frozen,
        panes: &inner.panes,
        engine: &inner.engine,
        listener: &*inner.listener,
    })
}

/// **What a command asks of the window,** whoever asked, carried out on the session thread: an agent's through its queue, as it runs on a thread of its own.
pub fn effects(core: Arc<crate::queue::CoreQueue>) -> recto_control::host::Effects {
    Arc::new(move |effect| core.push(crate::queue::CoreCall::Find(FindCall::Effect(effect))))
}

/// **The same for the person's command,** which runs on the session thread: carried out at once, in the call that ran it, as the window's own Find did.
pub fn effects_now(inner: &Arc<Inner>) -> recto_control::host::Effects {
    let inner = Arc::clone(inner);
    Arc::new(move |effect| carry_out(&inner, effect))
}

/// Find's hit selected and brought into view, or the person's step kept for when its hit is known.
fn carry_out(inner: &Inner, effect: Effect) {
    handle(inner, |find| find.carry_out(effect));
}

/// **What the window does with how the person's Find went:** why what they typed can't be found, on the line under the field, as a query too long; and a
/// step kept for its hit forgotten when Find closes ([`FindHandle::answered`]).
pub fn answered(inner: &std::sync::Arc<Inner>, ran: &person::Ran<'_>) {
    let Command::Move(Move::Find(asked)) = ran.command else {
        return;
    };
    // A query refused as too long, by the rule that refuses it: what was typed, or for ⌘F and a step what the field holds, which they find again (the query,
    // or the draft if there's none). Never by the refusal's words. Worked out before the field is locked, as it reads the session.
    let too_long = matches!(asked.ask, FindAsk::Query(_) | FindAsk::Open | FindAsk::Step { .. })
        && ran.refused().is_some_and(|error| error.code == Code::BAD_QUERY)
        && match &asked.ask {
            FindAsk::Query(query) => recto_agent::find::too_long_query(query),
            _ => inner.host.session.with_session(|s| {
                let find = s.viewing().find();
                recto_agent::find::too_long_query(if find.query.is_empty() { &find.draft } else { &find.query })
            }),
        };
    handle(inner, |find| find.answered(&asked.ask, too_long));
}

/// **Find's menu ids** ([`super::menu_rows`]): ⌘F is `find --open`, and then the field takes the keyboard; ⌘G and ⇧⌘G are `find --next|--previous`, a step.
pub fn row<'a>(_: &'a Arc<Inner>, asked: &super::menu_rows::Asked<'_>) -> Option<super::menu_rows::Row<'a>> {
    use super::menu_rows::Row;
    Some(match asked.action.as_ref()? {
        crate::menus::Action::FindOpen => Row::Command {
            command: opened(),
            when_run: None,
            host: super::HostAction::FocusFind,
        },
        crate::menus::Action::FindStep(forward) => Row::command(step(*forward, false)),
        _ => return None,
    })
}

/// What's in the Find field now, as it's typed: `find <query>`.
pub fn typed(text: String) -> Command {
    Find::from(FindAsk::Query(text)).into()
}

/// ⌘F, or the field's own button: `find --open`, which shows the sidebar Find is in.
pub fn opened() -> Command {
    Find::from(FindAsk::Open).into()
}

/// Escape in the field, or its close button: `find --close`.
pub fn closed() -> Command {
    Find::from(FindAsk::Close).into()
}

/// ⌘G and ⇧⌘G, a step, or Return and ⇧Return in the field, a jump, so Back comes here again: `find --next|--previous [--jump]`.
pub fn step(forward: bool, jump: bool) -> Command {
    Find::from(FindAsk::Step { forward, jump }).into()
}

/// A click on a row of Find's list: `find --hit N`, the hit that row is. A page's row is no hit, and does nothing.
pub fn chosen(find: &FindHandle<'_, dyn crate::Listener>, row: u32) -> Option<Command> {
    let index = find.hit_of_row(row).and_then(|index| u32::try_from(index).ok())?;
    Some(Find::from(FindAsk::Hit(index)).into())
}

pub fn is_open(inner: &Inner) -> bool {
    app::is_open(&inner.tools.find_field)
}

/// Find's heights in the sidebar, for this much room: its line wrapped at the sidebar's width as the session has it.
pub fn layout(inner: &Inner, room: f64) -> Heights {
    let shown = view(inner);
    let width = lock(&inner.window.app).sidebar_width;
    heights(room, shown.open, shown.row_count, app::status_lines(&shown.status, width))
}

pub fn view(inner: &Inner) -> FindView {
    handle(inner, |find| find.view())
}

pub fn rows(inner: &Inner, first: u32, count: u32) -> Vec<FindRow> {
    handle(inner, |find| find.rows(first, count)).into_iter().collect()
}

pub fn act(inner: &Inner, call: FindCall, events: &mut Vec<Event>) {
    handle(inner, |find| find.act(call, events));
}

pub fn keep_up(inner: &Inner) {
    handle(inner, |find| find.keep_up());
}

pub fn report(inner: &Inner) -> Value {
    handle(inner, |find| find.report())
}
