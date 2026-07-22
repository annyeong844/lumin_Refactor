const CRASH_POINT_ENV: &str = "LUMIN_TEST_RETENTION_CRASH_POINT";
const CRASH_EXIT_CODE: i32 = 93;
const INVALID_SELECTOR_EXIT_CODE: i32 = 94;

pub(super) enum RetentionCrashPoint {
    BeforePreparedCommit,
    BeforePruningCommit,
    AfterPruningCommit,
    PayloadMoved(usize),
    AfterMovesCommitted,
    AfterPrunedCommit,
    ReclaimChildRemoved(usize),
    AfterReclaimPayloadsFlushed,
    AfterReclaimAnchorRemoved,
    AfterReclaimDirectoryRemoved,
    AfterReclaimParentFlushed,
}

#[derive(Default)]
pub(super) struct CrashPointSequence {
    next_index: usize,
}

impl CrashPointSequence {
    pub(super) fn hit_indexed(&mut self, point: fn(usize) -> RetentionCrashPoint) {
        let index = self.next_index;
        let Some(next_index) = self.next_index.checked_add(1) else {
            eprintln!("retention test crash-point index overflowed");
            std::process::exit(INVALID_SELECTOR_EXIT_CODE);
        };
        self.next_index = next_index;
        hit(point(index));
    }
}

pub(super) fn hit(point: RetentionCrashPoint) {
    let Some(requested) = std::env::var_os(CRASH_POINT_ENV) else {
        return;
    };
    let Ok(requested) = requested.into_string() else {
        eprintln!("{CRASH_POINT_ENV} is not valid Unicode");
        std::process::exit(INVALID_SELECTOR_EXIT_CODE);
    };
    if !valid_selector(&requested) {
        eprintln!("unknown retention test crash point: {requested}");
        std::process::exit(INVALID_SELECTOR_EXIT_CODE);
    }
    if requested == point.label() {
        std::process::exit(CRASH_EXIT_CODE);
    }
}

impl RetentionCrashPoint {
    fn label(&self) -> String {
        match self {
            Self::BeforePreparedCommit => "before-prepared-commit".to_owned(),
            Self::BeforePruningCommit => "before-pruning-commit".to_owned(),
            Self::AfterPruningCommit => "after-pruning-commit".to_owned(),
            Self::PayloadMoved(index) => format!("after-payload-move-{index}"),
            Self::AfterMovesCommitted => "after-moves-committed".to_owned(),
            Self::AfterPrunedCommit => "after-pruned-commit".to_owned(),
            Self::ReclaimChildRemoved(index) => format!("after-reclaim-child-{index}"),
            Self::AfterReclaimPayloadsFlushed => "after-reclaim-payloads-flushed".to_owned(),
            Self::AfterReclaimAnchorRemoved => "after-reclaim-anchor-removed".to_owned(),
            Self::AfterReclaimDirectoryRemoved => "after-reclaim-directory-removed".to_owned(),
            Self::AfterReclaimParentFlushed => "after-reclaim-parent-flushed".to_owned(),
        }
    }
}

fn valid_selector(value: &str) -> bool {
    matches!(
        value,
        "before-prepared-commit"
            | "before-pruning-commit"
            | "after-pruning-commit"
            | "after-moves-committed"
            | "after-pruned-commit"
            | "after-reclaim-payloads-flushed"
            | "after-reclaim-anchor-removed"
            | "after-reclaim-directory-removed"
            | "after-reclaim-parent-flushed"
    ) || indexed_selector(value, "after-payload-move-")
        || indexed_selector(value, "after-reclaim-child-")
}

fn indexed_selector(value: &str, prefix: &str) -> bool {
    value
        .strip_prefix(prefix)
        .is_some_and(|index| !index.is_empty() && index.parse::<usize>().is_ok())
}
