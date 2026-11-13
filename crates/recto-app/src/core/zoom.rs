//! **The zoom, `view --zoom`** (step R2.3, C5a): about what's on a screen, so the page views' and never the session's. One handler for both actors: an agent's
//! command through [`AppHost`], and the person's ⌘+ and ⌘-, Actual Size, the fit and the level typed in the top left, run on the thread that asked (the UI
//! thread, for the person), taking no session lock and queuing nothing.

use super::*;
use recto_agent::Code;
use recto_agent::commands::view::View;
use recto_reader::tiles::Zoom;

/// **Zooms every pane as `asked.zoom` says,** and says the zoom now, as a magnification of the fitted width, and its level as the top left shows it.
///
/// # Errors
///
/// A word that isn't a zoom, or no page to zoom.
pub fn run(host: &AppHost, asked: &View) -> Result<Value, AgentError> {
    let word = asked.zoom.as_deref().unwrap_or_default();
    let zoom = menus::zoom_from_word(word).ok_or_else(|| {
        AgentError::new(
            Code::USAGE,
            format!(
                "`{}` isn't a zoom. It's `in`, `out`, `fit`, `actual` or a number.",
                recto_core::clean_line(word)
            ),
            recto_agent::exit::USAGE,
        )
    })?;
    let inner = host.saver.inner();
    let reading = inner
        .as_ref()
        .map_or_else(|| host.engine.clone(), |inner| inner.panes.reading_engine(&inner.engine));
    let value = inner
        .as_ref()
        .map_or_else(|| host.engine.zoom(zoom), |inner| inner.panes.zoom_all(&inner.engine, zoom))
        .ok_or_else(|| AgentError::new(Code::NO_ACTIVE_TAB, "There's no page to zoom.", recto_agent::exit::NOT_FOUND))?;
    let mut answer = json!({ "zoom": value });
    if let Some(fit) = reading.fit() {
        answer["zoom_level"] = json!(menus::zoom_label(value, fit).text);
    }
    Ok(answer)
}

/// The word `view --zoom` takes for a zoom, which [`menus::zoom_from_word`] reads back as the same zoom.
pub fn word(zoom: Zoom) -> String {
    match zoom {
        Zoom::In => "in".into(),
        Zoom::Out => "out".into(),
        Zoom::Fit => "fit".into(),
        Zoom::Actual => "actual".into(),
        Zoom::To(value) => value.to_string(),
        Zoom::Percent(percent) => format!("{percent}%"),
    }
}

/// **The zoom's menu ids** ([`super::menu_rows`]): `view --zoom`, the person's run here and now, on the thread that asked.
pub fn row<'a>(inner: &'a Arc<Inner>, asked: &super::menu_rows::Asked<'_>) -> Option<super::menu_rows::Row<'a>> {
    let Some(menus::Action::Zoom(zoom)) = asked.action else { return None };
    let command = View {
        zoom: Some(word(zoom)),
        ..View::default()
    };
    Some(super::menu_rows::Row::own_way(
        command,
        "the zoom is the page views', so the person's is run on the thread that asked, taking no lock and queuing nothing",
        move || {
            inner.zoom_for_person(zoom);
            HostAction::None
        },
    ))
}

impl Inner {
    /// **The person's zoom:** `view --zoom`, run for them here and now by the same handler as an agent's. Says whether there was a page to zoom.
    pub fn zoom_for_person(&self, zoom: Zoom) -> bool {
        let asked = View {
            zoom: Some(word(zoom)),
            ..View::default()
        };
        super::person::sent(self, &asked.clone().into());
        run(&self.host, &asked).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Failure: a zoom the person asks for comes to a word `view --zoom` reads as another zoom, or as none: a typed level of `150,5` (with the Mac's comma)
    /// zooms to 1505%, or a long one isn't taken. Each kind, and typed levels as long as the field takes, come back as they went.
    #[test]
    fn a_persons_zoom_is_the_word_view_reads_back() {
        let typed = ["150", "150,5", "33,333333333", "1 234,5", "1.234,5", "9999999999,5", "0,1"];
        let zooms = [Zoom::In, Zoom::Out, Zoom::Fit, Zoom::Actual, Zoom::To(2.5), Zoom::To(1e-3)]
            .into_iter()
            .chain(typed.iter().map(|text| recto_reader::tiles::typed_zoom(text, ',').expect("a level")));
        for zoom in zooms {
            assert_eq!(menus::zoom_from_word(&word(zoom)), Some(zoom), "{zoom:?} as {}", word(zoom));
        }
    }
}
