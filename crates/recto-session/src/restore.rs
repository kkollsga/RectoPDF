//! The session as a record and back.

use super::*;

impl<L: DocLoader> Session<L> {
    /// The whole session as a record, with the host's own part at its defaults for the host to fill in. Every field is named here, with no `..`, so one added to a tab or to the view doesn't compile until it's carried or said not to be. A tab's edits go by journal, and [`Self::edits_record`] gives them whole.
    pub fn to_record(&self) -> SessionRecord {
        let Self {
            loader: _,
            viewing,
            tabs: open,
            warm: _,
            lists,
            dormancy: _,
            mail: _,
            journals,
            generated: _,
            comparison,
            saves: _,
            recent,
        } = self;
        let tabs = open
            .all()
            .iter()
            .map(|tab| {
                let TabRecord {
                    from_sleep: _,
                    touched: _,
                    id,
                    name: _,
                    page,
                    page_count,
                    toc_open,
                    problem: _,
                    renderer_only,
                    view_rotation,
                    spread,
                    place_dirty: _,
                    contents_verdict: _,
                    generated_contents,
                    folds,

                    file:
                        TabFile {
                            dir: _,
                            file_name: _,
                            stamp,
                            news: _,
                            encrypted: _,
                            password,
                        },
                    reading: TabPlace { place, selection, trail },
                } = tab;
                TabEntry {
                    id: id.0,
                    path: PathField::from_path(&self.tabs.named_path(*id).unwrap_or_default()),
                    file: stamp.map(Into::into),
                    page: *page,
                    page_count: *page_count,
                    place: *place,
                    view_rotation: *view_rotation,
                    toc_open: *toc_open,
                    layout: *spread,
                    renderer_only: *renderer_only,
                    selection: selection.as_deref().copied(),
                    // Edits on a version the file isn't any more are said to be whatever notice is over them, as the tab says it: so a restore knows.
                    notice: match self.notices().fit(*id) {
                        EditsFit::FileChanged | EditsFit::FileChangedVersionUnknown => Some(FileNotice::Changed),
                        _ => tab.file.notice().cloned(),
                    },
                    journal: journals.name_of(*id).map(ToString::to_string),
                    edits: lists.summary(*id),
                    generated_contents: *generated_contents,
                    contents_folds: folds.as_deref().map(<[(u32, bool)]>::to_vec).unwrap_or_default(),
                    password: *password,
                    back: trail.as_ref().map(|trail| trail.back().to_vec()).unwrap_or_default(),
                    forward: trail.as_ref().map(|trail| trail.forward().to_vec()).unwrap_or_default(),
                }
            })
            .collect();
        let comparison = comparison.as_ref().map(|comparison| record::ComparisonRecord {
            tabs: comparison.tabs.iter().map(|tab| tab.0).collect(),
            synced: comparison.synced,
            sync_from: comparison.sync_from.clone(),
        });
        let recent = recent.to_record();
        let (view, find, edit_mode) = viewing.to_record();
        SessionRecord {
            app: record::AppRecord::default(),
            view,
            find,
            edit_mode,
            active: open.active().map(|id| id.0),
            next_id: open.next_id(),
            tabs,
            comparison,
            recent,
        }
    }

    /// **Puts the session back as a record has it.** No file is opened: every tab comes back cold, and its name and place are all it costs. Replaces what's here. The caller reads journals, and gets the active tab's file read, and checks what's become of the rest, each off this lock.
    pub fn restore(&mut self, record: &SessionRecord, mut journals: HashMap<String, RestoredEdits>) -> Result<(), RecordError> {
        record::validate_record(record)?;
        // A PDF is open once, even if the record says otherwise.
        let mut deduped = record.clone();
        record::one_tab_per_file(&mut deduped);
        let record = &deduped;
        self.tabs.restored(record.next_id, record.tabs.len());
        self.warm = warm::Warm::default();
        self.lists.restored();
        self.mail = mail::PageViewMail::default();
        self.journals.restored();
        // Each of these is about a tab of the session being replaced, and a restored tab can have its number: another file's generated contents, sleep file or save would be taken as its own. What's still to be done on disk, journals to write or remove, stays, as it's about files.
        self.generated = generated::Generated::default();
        self.saves.restored();
        self.dormancy.restored();
        self.comparison = None;
        self.recent.restore(&record.recent);
        self.viewing.restore(&record.view, &record.find, record.edit_mode);
        for entry in &record.tabs {
            let Some(path) = entry.path.to_path() else { continue };
            // A quiet run's record naming a file outside its folders doesn't bring that tab back: see [`confined`].
            if !self.tabs.confinement().admits(&path) {
                continue;
            }
            let dir = self.tabs.intern_dir(path.parent().unwrap_or(Path::new("")));
            let Some(file_name) = path.file_name() else { continue };
            let id = TabId(entry.id);
            let mut notice = entry.notice.clone();
            if let Some(name) = &entry.journal {
                // A list naming a file outside a confined session's folders is as unreadable as one that's gone: see [`confined`].
                match journals.remove(name).filter(|restored| self.tabs.confinement().admits_edits(&restored.edits)) {
                    Some(restored) => {
                        if restored.rolled_off {
                            notice = Some(FileNotice::HistoryRolledOff);
                        }
                        if restored.lost_lines {
                            notice = Some(FileNotice::EditsUnreadable);
                        }
                        let mut edits = restored.edits;
                        if self.journals.journaling() {
                            edits.set_journal_seq(restored.seq);
                        } else {
                            edits.no_journal();
                        }
                        self.lists.insert(id, edits);
                        self.journals.set_name(id, name);
                    }
                    None => notice = Some(FileNotice::EditsUnreadable),
                }
            }
            let pages = self.lists.get(id).map_or(entry.page_count, PageEdits::len);
            // Edits the record says the file changed under aren't taken to fit it until a look or a read says so: nothing reads it as theirs meanwhile.
            let elsewhere = (matches!(entry.notice, Some(FileNotice::Changed)) && self.lists.is_edited(id)).then_some(fit::Elsewhere {
                seen: None,
                seen_pages: None,
                file_changed: true,
            });
            let trail = (!entry.back.is_empty() || !entry.forward.is_empty()).then(|| Box::new(trail::Trail::from_record(&entry.back, &entry.forward, pages)));
            self.tabs.push(TabRecord {
                from_sleep: None,
                touched: 0,
                id,
                name: tab_name(&path).into(),
                page: entry.page.min(pages.saturating_sub(1)),
                page_count: entry.page_count,
                toc_open: entry.toc_open,
                problem: None,
                renderer_only: entry.renderer_only,
                view_rotation: entry.view_rotation,
                spread: entry.layout,
                place_dirty: false,
                contents_verdict: ContentsVerdict::Fine,
                generated_contents: entry.generated_contents,
                folds: (!entry.contents_folds.is_empty()).then(|| entry.contents_folds.iter().copied().take(record::MAX_CONTENTS_FOLDS).collect()),
                file: TabFile {
                    dir,
                    file_name: file_name.into(),
                    stamp: entry.file.map(Into::into),
                    news: fit::FileNews::restored(notice, elsewhere),
                    encrypted: false,
                    password: entry.password,
                },
                reading: TabPlace {
                    place: entry.place,
                    selection: entry.selection.map(Box::new),
                    trail,
                },
            });
        }
        self.tabs.set_active(record.active.map(TabId).filter(|id| self.tabs.tab(*id).is_some()));
        // Side by side as it was, of the tabs that came back. A record's been checked, but one tab per file can take one out.
        self.comparison = record.comparison.as_ref().and_then(|saved| {
            let mut comparison = compare::Comparison::new(saved.tabs.iter().copied().map(TabId).filter(|id| self.tabs.tab(*id).is_some()).collect()).ok()?;
            if saved.synced && saved.sync_from.len() == saved.tabs.len() {
                comparison.synced = true;
                comparison.sync_from = saved
                    .tabs
                    .iter()
                    .zip(&saved.sync_from)
                    .filter(|(id, _)| comparison.contains(TabId(**id)))
                    .map(|(_, page)| *page)
                    .collect();
            }
            Some(comparison)
        });
        if let Some(comparison) = &self.comparison
            && !self.tabs.active().is_some_and(|active| comparison.contains(active))
        {
            self.tabs.set_active(comparison.tabs.first().copied());
        }
        // What's come back for tabs nobody's looking at is on disk already, and needn't be here too.
        if self.journals.kept_and_readable() {
            let others: Vec<TabId> = self.lists.ids().filter(|id| !self.on_screen(*id)).collect();
            for id in others {
                self.put_list_to_sleep(id);
            }
        }
        Ok(())
    }
}
