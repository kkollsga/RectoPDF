//! **The sidebar's Find: its records, the person's controls, and what a command asks of it** (step R2.3, C5g). Typing in the field, ⌘F, Escape, ⌘G and
//! ⇧⌘G, Return and ⇧Return and a click on a hit's row are `find <query>`, `find --open`, `find --close`, `find --next|--previous [--jump]` and
//! `find --hit N`, the command an agent sends, run for the person by the one executor. What it shows and when the host is told are `crate::find`'s; this
//! hands it the app's parts it reaches, as a [`FindHandle`], and converts what comes back.

use super::{Inner, lock, person};
use crate::find::{self as app, FindHandle};
pub use crate::find::{FIELD_HEIGHT, FindCall, ROW_HEIGHT, STATUS_HEIGHT, heights};
pub use crate::find::{FindRow, FindRowKind, FindView};
use recto_agent::commands::{Move, finding::Find};
use recto_agent::{Command, Effect, Event};
use serde_json::Value;
use std::sync::Arc;

/// What Find reaches of the app, for one call.
fn handle<R>(inner: &Inner, step: impl FnOnce(&FindHandle<'_, dyn crate::Listener>) -> R) -> R {
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
/// step kept for its hit forgotten when Find closes. Then the host's told, as Find's view isn't in what `reconcile` compares.
pub fn answered(inner: &std::sync::Arc<Inner>, ran: &person::Ran<'_>) {
    let Command::Move(Move::Find(asked)) = ran.command else {
        return;
    };
    {
        let mut field = lock(&inner.tools.find_field);
        if asked.close {
            field.clear_pending();
        } else if asked.query.is_some() || asked.open || asked.next || asked.previous {
            let too_long = recto_agent::find::too_long();
            field.set_note(
                ran.refused()
                    .filter(|error| error.code == too_long.code && error.message == too_long.message)
                    .map(|_| too_long.message),
            );
        }
    }
    inner.listener.model_changed();
}

/// **Find's menu ids** ([`super::menu_rows`]): ⌘F is `find --open`, and then the field takes the keyboard; ⌘G and ⇧⌘G are `find --next|--previous`, a step.
pub fn row<'a>(_: &'a Arc<Inner>, asked: &super::menu_rows::Asked<'_>) -> Option<super::menu_rows::Row<'a>> {
    use super::menu_rows::Row;
    Some(match asked.action.as_ref()? {
        crate::menus::Action::FindOpen => Row::Command {
            command: Find { open: true, ..Find::default() }.into(),
            when_run: None,
            host: super::HostAction::FocusFind,
        },
        crate::menus::Action::FindStep(forward) => Row::command(Find {
            next: *forward,
            previous: !*forward,
            ..Find::default()
        }),
        _ => return None,
    })
}

impl Inner {
    /// What's in the Find field now, as it's typed: `find <query>`.
    pub fn find_typed_for_person(self: &Arc<Self>, text: String) {
        person::submit(
            self,
            Find {
                query: Some(text),
                ..Find::default()
            }
            .into(),
        );
    }

    /// ⌘F, or the field's own button: `find --open`, which shows the sidebar Find is in.
    pub fn find_open_for_person(self: &Arc<Self>) {
        person::submit(self, Find { open: true, ..Find::default() }.into());
    }

    /// Escape in the field, or its close button: `find --close`.
    pub fn find_close_for_person(self: &Arc<Self>) {
        person::submit(
            self,
            Find {
                close: true,
                ..Find::default()
            }
            .into(),
        );
    }

    /// ⌘G and ⇧⌘G, a step, or Return and ⇧Return in the field, a jump, so Back comes here again: `find --next|--previous [--jump]`.
    pub fn find_step_for_person(self: &Arc<Self>, forward: bool, jump: bool) {
        person::submit(
            self,
            Find {
                next: forward,
                previous: !forward,
                jump,
                ..Find::default()
            }
            .into(),
        );
    }

    /// A click on a row of Find's list: `find --hit N`, the hit that row is. A page's row is no hit, and does nothing.
    pub fn find_choose_for_person(self: &Arc<Self>, row: u32) {
        if let Some(index) = handle(self, |find| find.hit_of_row(row)).and_then(|index| u32::try_from(index).ok()) {
            person::submit(
                self,
                Find {
                    hit: Some(index),
                    ..Find::default()
                }
                .into(),
            );
        }
    }
}

pub fn is_open(inner: &Inner) -> bool {
    app::is_open(&inner.tools.find_field)
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
