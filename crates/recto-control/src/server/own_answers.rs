//! **The commands the server's loop answers itself** (route `Server`): `stop`, `status`, the permissions, the log, `wait`, `watch` and `watch stop`. It holds the connections, the level and the log, so these never reach the host. Everything else is sent on by [`Loop::send_on`].

use super::*;
use recto_agent::Code;

impl<H: Host> Loop<H> {
    /// **The commands the server answers itself:** it holds the connections, the level and the log. Every Server command has its arm; a command declared the server's with none here is sent on, and the host says it needs the long-running session, which the routing guard catches.
    pub(super) fn answer_here(&mut self, index: usize, level: Level, command: Command) {
        let tag = command.tag();
        match command {
            Command::Session(Session::Stop(_)) => {
                self.conns[index].state = State::Busy;
                let conn = self.conns[index].id;
                self.stop_request = Some(StopRequest {
                    conn,
                    deadline: Instant::now() + self.limits.stop_grace,
                    checking: false,
                });
                // Anything still waiting its turn could be an edit that `stop` would then lose.
                let queued: Vec<u64> = self.queue.iter().filter(|job| matches!(job, Job::Command { .. })).map(Job::conn).collect();
                self.queue.retain(|job| !matches!(job, Job::Command { .. }));
                for conn in queued {
                    if let Some(index) = self.conns.iter().position(|c| c.id == conn) {
                        self.answer(index, "unknown", &Err(stopping_now()));
                    }
                }
            }
            Command::Start(Start::Status(_)) => {
                let data = self.status(level);
                self.log.record(tag, level, "ok", &Value::Null);
                self.answer(index, tag, &Ok(data));
            }
            Command::Rights(Rights::LevelChanged(_)) => self.answer(index, tag, &Ok(json!({ "level": level.name() }))),
            Command::Rights(Rights::PermissionShow(_)) => {
                let data = levels::show(&self.paths, self.pending);
                self.log.record(tag, level, "ok", &Value::Null);
                self.answer(index, tag, &Ok(data));
            }
            Command::Rights(Rights::PermissionRequest(asked)) => self.permission_request(index, level, asked.level, asked.timeout_ms),
            Command::Waits(Waits::Wait(Wait { tab, .. })) if tab.over_long() => {
                self.answer(index, tag, &Err(AgentError::new(Code::NO_SUCH_TAB, "That tab isn't open.", exit::NOT_FOUND)));
            }
            Command::Waits(Waits::Wait(_) | Waits::Watch(_)) if self.subscribers() >= self.limits.max_subscribers => {
                self.log.record(tag, level, "busy", &Value::Null);
                self.answer(index, tag, &Err(too_many_waiting()));
            }
            Command::Waits(Waits::Wait(wait)) => {
                let deadline = Instant::now() + Duration::from_millis(wait.timeout_ms.min(MAX_WAIT_MS));
                self.log.record(tag, level, "started", &json!({ "condition": format!("{:?}", wait.condition) }));
                let generation = self.next_generation();
                self.conns[index].state = State::Waiting {
                    what: WaitFor::Condition {
                        condition: wait.condition,
                        tab: wait.tab,
                        page: wait.page,
                    },
                    tag,
                    deadline,
                    generation,
                    looking: false,
                    look_again: false,
                };
                self.look(index);
            }
            Command::Waits(Waits::Watch(Watch { events, timeout_ms, count })) => self.watch(index, level, events, timeout_ms, count),
            Command::Waits(Waits::WatchStop(_)) => {
                let watching: Vec<(usize, u64)> = self
                    .conns
                    .iter()
                    .enumerate()
                    .filter_map(|(at, conn)| match conn.state {
                        State::Watching { delivered, .. } => Some((at, delivered)),
                        _ => None,
                    })
                    .collect();
                for (at, delivered) in &watching {
                    self.end_watch(*at, "ended by `recto watch stop`", *delivered);
                }
                let stopped = watching.len();
                self.log.record(tag, level, "ok", &json!({ "stopped": stopped }));
                self.answer(index, tag, &Ok(json!({ "stopped": stopped })));
            }
            Command::Rights(Rights::Log(Log { last })) => {
                self.log.flush();
                let data = log::recent(&self.paths, last);
                self.log.record(tag, level, "ok", &Value::Null);
                self.answer(index, tag, &Ok(data));
            }
            Command::Rights(Rights::Sign(_))
            | Command::Measure(_)
            | Command::Edit(_)
            | Command::Move(_)
            | Command::Read(_)
            | Command::Qa(_)
            | Command::Record(_)
            | Command::Updates(_) => self.send_on(index, command),
        }
    }
}
