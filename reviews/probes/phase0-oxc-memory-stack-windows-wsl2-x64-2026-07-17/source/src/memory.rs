use anyhow::Result;

use crate::model::MemoryObservation;

#[cfg(windows)]
pub fn observe() -> Result<MemoryObservation> {
    use anyhow::Context;
    use windows_sys::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    let mut counters = PROCESS_MEMORY_COUNTERS::default();
    // SAFETY: the current-process pseudo-handle is valid and `counters` has the advertised size.
    let result = unsafe {
        GetProcessMemoryInfo(
            GetCurrentProcess(),
            &mut counters,
            u32::try_from(std::mem::size_of::<PROCESS_MEMORY_COUNTERS>())?,
        )
    };
    if result == 0 {
        return Err(std::io::Error::last_os_error()).context("read process memory counters");
    }
    Ok(MemoryObservation {
        current_rss_bytes: Some(counters.WorkingSetSize as u64),
        peak_rss_bytes: Some(counters.PeakWorkingSetSize as u64),
    })
}

#[cfg(target_os = "linux")]
pub fn observe() -> Result<MemoryObservation> {
    let status = std::fs::read_to_string("/proc/self/status")?;
    let value = |prefix: &str| -> Result<Option<u64>> {
        status
            .lines()
            .find_map(|line| line.strip_prefix(prefix))
            .and_then(|value| value.split_whitespace().next())
            .map(str::parse::<u64>)
            .transpose()
            .map(|value| value.map(|kilobytes| kilobytes * 1024))
            .map_err(Into::into)
    };
    Ok(MemoryObservation {
        current_rss_bytes: value("VmRSS:")?,
        peak_rss_bytes: value("VmHWM:")?,
    })
}

#[cfg(not(any(windows, target_os = "linux")))]
pub fn observe() -> Result<MemoryObservation> {
    Ok(MemoryObservation {
        current_rss_bytes: None,
        peak_rss_bytes: None,
    })
}
