//! CPU used by one exchange, rather than by the worker's earlier jobs. Lives only while a job is active.

use std::cell::Cell;
use std::time::Duration;

#[derive(Default)]
pub(super) struct JobCpu(Cell<Option<(u32, u64)>>);

impl JobCpu {
    /// Reset for every frame, including a hidden-set frame sent ahead of the requested job.
    pub(super) fn begin(&self, reading: Option<(u32, u64)>) {
        self.0.set(reading);
    }

    /// Unknown readings remain unknown; neither another worker nor a decreasing counter proves starvation.
    pub(super) fn starved(&self, reading: Option<(u32, u64)>, deadline: Duration) -> Option<bool> {
        let (before_pid, before) = self.0.get()?;
        let (after_pid, after) = reading?;
        if before_pid != after_pid {
            return None;
        }
        let cpu = after.checked_sub(before)?;
        Some(u128::from(cpu) < deadline.as_nanos() / 2)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Failure: CPU spent on earlier jobs makes a reused worker's almost-unrun current job look as though it had its share of the processor.
    #[test]
    fn earlier_jobs_cpu_cannot_mask_a_starved_exchange() {
        let cpu = JobCpu::default();
        let prior = 30_000_000_000;
        cpu.begin(Some((42, prior)));
        assert_eq!(cpu.starved(Some((42, prior + 1_000)), Duration::from_secs(5)), Some(true));
        assert_eq!(cpu.starved(Some((42, prior + 2_500_000_000)), Duration::from_secs(5)), Some(false));
        cpu.begin(Some((42, prior + 2_500_000_000)));
        assert_eq!(cpu.starved(Some((42, prior + 2_500_001_000)), Duration::from_secs(5)), Some(true));
    }

    /// Failure: a missing, decreasing or different worker's counter invents zero current CPU and wrongly excuses a deadline kill as starvation.
    #[test]
    fn an_unknown_or_replaced_job_counter_stays_unknown() {
        let cpu = JobCpu::default();
        let deadline = Duration::from_secs(5);
        assert_eq!(cpu.starved(Some((42, 0)), deadline), None);
        cpu.begin(Some((42, 10)));
        assert_eq!(cpu.starved(None, deadline), None);
        assert_eq!(cpu.starved(Some((42, 9)), deadline), None);
        assert_eq!(cpu.starved(Some((43, 11)), deadline), None);
        cpu.begin(None);
        assert_eq!(cpu.starved(Some((42, 11)), deadline), None);
        cpu.begin(Some((43, 100)));
        assert_eq!(cpu.starved(Some((43, 100)), deadline), Some(true));
    }
}
