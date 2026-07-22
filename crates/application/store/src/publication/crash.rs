const CRASH_POINT_ENV: &str = "LUMIN_TEST_PUBLICATION_CRASH_POINT";
const CRASH_EXIT_CODE: i32 = 95;
const INVALID_SELECTOR_EXIT_CODE: i32 = 96;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PublicationCrashPoint {
    BeforeAttemptCatalogAllocation,
    AfterCatalogAllocation,
    AfterRunningEnvelope,
    AfterLatestRunning,
    AfterRunRename,
    AfterTerminalAttempt,
    AfterLatestTemp,
    AfterLatestReplace,
}

impl PublicationCrashPoint {
    fn label(self) -> &'static str {
        match self {
            Self::BeforeAttemptCatalogAllocation => "before-attempt-catalog-allocation",
            Self::AfterCatalogAllocation => "after-catalog-allocation",
            Self::AfterRunningEnvelope => "after-running-envelope",
            Self::AfterLatestRunning => "after-latest-running",
            Self::AfterRunRename => "after-run-rename",
            Self::AfterTerminalAttempt => "after-terminal-attempt",
            Self::AfterLatestTemp => "after-latest-temp",
            Self::AfterLatestReplace => "after-latest-replace",
        }
    }
}

pub(super) fn hit(point: PublicationCrashPoint) {
    let Ok(requested) = std::env::var(CRASH_POINT_ENV) else {
        return;
    };
    if !ALL_POINTS
        .iter()
        .any(|candidate| candidate.label() == requested)
    {
        eprintln!("unknown publication test crash point: {requested}");
        std::process::exit(INVALID_SELECTOR_EXIT_CODE);
    }
    if requested == point.label() {
        std::process::exit(CRASH_EXIT_CODE);
    }
}

const ALL_POINTS: [PublicationCrashPoint; 8] = [
    PublicationCrashPoint::BeforeAttemptCatalogAllocation,
    PublicationCrashPoint::AfterCatalogAllocation,
    PublicationCrashPoint::AfterRunningEnvelope,
    PublicationCrashPoint::AfterLatestRunning,
    PublicationCrashPoint::AfterRunRename,
    PublicationCrashPoint::AfterTerminalAttempt,
    PublicationCrashPoint::AfterLatestTemp,
    PublicationCrashPoint::AfterLatestReplace,
];
