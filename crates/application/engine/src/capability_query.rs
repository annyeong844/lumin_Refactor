use std::collections::BTreeMap;

use lumin_evidence::{
    CapabilityRecord, CollectionOrderingId, DEAD_CODE_CAPABILITY_ID, EvidencePage, EvidenceQuery,
    EvidenceQueryScope, RunEvidence,
};
use lumin_model::{BuildIdentity, CapabilityState, RepositoryId, RunId, digest_hex};

use crate::EngineError;
use crate::gate_query::{EvidenceQueryError, page, validated_query};

const CAPABILITIES_PAGE_SIZE: usize = 3;
const BINARY_CAPABILITIES_PATH: &str = "binary/capabilities";
const RUN_CAPABILITIES_PATH: &str = "run/capabilities";

/// The compiled capability registry built at binary construction time.
#[derive(Clone, Debug)]
pub struct CompiledCapabilityRegistry {
    rows: Vec<CapabilityRecord>,
    contract_digest: String,
}

impl CompiledCapabilityRegistry {
    pub fn rows(&self) -> &[CapabilityRecord] {
        &self.rows
    }

    pub fn contract_digest(&self) -> &str {
        &self.contract_digest
    }
}

/// Build the compiled capability registry from 4 real rows: dead-code Complete + SFC owner rows.
/// Canonicalizes by capability_id, fails hard on duplicate IDs.
pub fn compiled_capability_registry() -> Result<CompiledCapabilityRegistry, EngineError> {
    let mut rows = Vec::with_capacity(4);

    // dead-code Complete
    rows.push(CapabilityRecord {
        capability_id: DEAD_CODE_CAPABILITY_ID.to_owned(),
        state: CapabilityState::Complete,
    });

    // SFC owner rows from the sfc crate's compiled dialect states
    for (_dialect, capability_id, state) in lumin_sfc::compiled_dialect_states() {
        rows.push(CapabilityRecord {
            capability_id: capability_id.to_owned(),
            state,
        });
    }

    let rows = canonical_capabilities(rows)?;

    // The registry identity is semantic: sorted IDs and explicit model state tags.
    let mut digest_input = Vec::new();
    lumin_model::append_length_prefixed(
        &mut digest_input,
        b"lumin-capability-registry-contract.v1",
    );
    for row in &rows {
        lumin_model::append_length_prefixed(&mut digest_input, row.capability_id.as_bytes());
        lumin_model::append_length_prefixed(
            &mut digest_input,
            capability_state_tag(row.state).as_bytes(),
        );
    }
    let contract_digest = digest_hex(&digest_input);

    Ok(CompiledCapabilityRegistry {
        rows,
        contract_digest,
    })
}

fn canonical_capabilities(
    mut rows: Vec<CapabilityRecord>,
) -> Result<Vec<CapabilityRecord>, EvidenceQueryError> {
    rows.sort_by(|left, right| left.capability_id.cmp(&right.capability_id));
    if let Some(duplicate) = rows
        .windows(2)
        .find(|window| window[0].capability_id == window[1].capability_id)
    {
        return Err(EvidenceQueryError::DuplicateCapabilityId(
            duplicate[0].capability_id.clone(),
        ));
    }
    Ok(rows)
}

fn capability_state_tag(state: CapabilityState) -> &'static str {
    match state {
        CapabilityState::Complete => "complete",
        CapabilityState::Incomplete => "incomplete",
        CapabilityState::Unavailable => "unavailable",
        CapabilityState::Failed => "failed",
    }
}

/// Query capabilities at the binary scope (no .lumin, repository-independent).
pub fn query_binary_capabilities(
    build_identity: &BuildIdentity,
    registry: &CompiledCapabilityRegistry,
    cursor: Option<EvidenceQuery>,
) -> Result<EvidencePage<CapabilityRecord>, EngineError> {
    let expected = EvidenceQuery {
        scope: EvidenceQueryScope::Binary {
            build_identity: build_identity.clone(),
        },
        finding_id: None,
        collection_path: BINARY_CAPABILITIES_PATH.to_owned(),
        ordering: CollectionOrderingId::capabilities(),
        page_size: CAPABILITIES_PAGE_SIZE,
        filters: BTreeMap::new(),
        anchor: None,
    };
    let query = validated_query(cursor, expected)?;
    page(&registry.rows, query, |record| {
        record.capability_id.as_str()
    })
    .map_err(Into::into)
}

/// Query capabilities at the run scope (persisted under .lumin).
pub fn query_run_capabilities(
    repository_id: &RepositoryId,
    run_id: &RunId,
    evidence: &RunEvidence,
    cursor: Option<EvidenceQuery>,
) -> Result<EvidencePage<CapabilityRecord>, EngineError> {
    let expected = EvidenceQuery {
        scope: EvidenceQueryScope::Run {
            repository_id: repository_id.clone(),
            run_id: run_id.clone(),
        },
        finding_id: None,
        collection_path: RUN_CAPABILITIES_PATH.to_owned(),
        ordering: CollectionOrderingId::capabilities(),
        page_size: CAPABILITIES_PAGE_SIZE,
        filters: BTreeMap::new(),
        anchor: None,
    };
    // Validate persisted collection integrity before interpreting continuation input.
    let sorted = canonical_capabilities(evidence.capabilities.clone())?;
    let query = validated_query(cursor, expected)?;
    page(&sorted, query, |record| record.capability_id.as_str()).map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_has_exactly_four_sorted_rows() -> Result<(), Box<dyn std::error::Error>> {
        let registry = compiled_capability_registry()?;
        assert_eq!(registry.rows.len(), 4);
        // Verify sorted by capability_id
        for window in registry.rows.windows(2) {
            assert!(window[0].capability_id < window[1].capability_id);
        }
        // Verify specific expected rows
        assert!(
            registry
                .rows
                .iter()
                .any(|r| r.capability_id == "dead-code.v1" && r.state == CapabilityState::Complete)
        );
        assert!(
            registry
                .rows
                .iter()
                .any(|r| r.capability_id == "sfc/vue.v1" && r.state == CapabilityState::Complete)
        );
        assert!(
            registry
                .rows
                .iter()
                .any(|r| r.capability_id == "sfc/svelte.v1"
                    && r.state == CapabilityState::Unavailable)
        );
        assert!(
            registry
                .rows
                .iter()
                .any(|r| r.capability_id == "sfc/astro.v1"
                    && r.state == CapabilityState::Unavailable)
        );
        Ok(())
    }

    #[test]
    fn registry_contract_digest_is_deterministic() -> Result<(), Box<dyn std::error::Error>> {
        let a = compiled_capability_registry()?;
        let b = compiled_capability_registry()?;
        assert_eq!(a.contract_digest, b.contract_digest);
        assert!(!a.contract_digest.is_empty());
        Ok(())
    }

    #[test]
    fn duplicate_capability_id_fails_hard() {
        let rows = vec![
            CapabilityRecord {
                capability_id: "test.v1".to_owned(),
                state: CapabilityState::Complete,
            },
            CapabilityRecord {
                capability_id: "test.v1".to_owned(),
                state: CapabilityState::Unavailable,
            },
        ];
        assert!(matches!(
            canonical_capabilities(rows),
            Err(EvidenceQueryError::DuplicateCapabilityId(id)) if id == "test.v1"
        ));
    }

    #[test]
    fn persisted_duplicate_fails_before_cursor_validation() {
        let repository_id = RepositoryId::from_string("repository-a".to_owned());
        let run_id = RunId::from_string("run-a".to_owned());
        let evidence = RunEvidence {
            schema_version: "lumin-evidence.v1".to_owned(),
            capabilities: vec![
                CapabilityRecord {
                    capability_id: "duplicate.v1".to_owned(),
                    state: CapabilityState::Complete,
                },
                CapabilityRecord {
                    capability_id: "duplicate.v1".to_owned(),
                    state: CapabilityState::Unavailable,
                },
            ],
            resolution_profiles: Vec::new(),
            findings: Vec::new(),
            limitations: Vec::new(),
        };
        let malformed_cursor = EvidenceQuery {
            scope: EvidenceQueryScope::Run {
                repository_id: repository_id.clone(),
                run_id: run_id.clone(),
            },
            finding_id: None,
            collection_path: "wrong/collection".to_owned(),
            ordering: CollectionOrderingId::capabilities(),
            page_size: CAPABILITIES_PAGE_SIZE,
            filters: BTreeMap::new(),
            anchor: Some(lumin_evidence::PageAnchor::from_string(
                "duplicate.v1".to_owned(),
            )),
        };

        assert!(matches!(
            query_run_capabilities(
                &repository_id,
                &run_id,
                &evidence,
                Some(malformed_cursor)
            ),
            Err(EngineError::EvidenceQuery(
                EvidenceQueryError::DuplicateCapabilityId(id)
            )) if id == "duplicate.v1"
        ));
    }

    #[test]
    fn binary_capabilities_pagination_3_plus_1() -> Result<(), Box<dyn std::error::Error>> {
        let registry = compiled_capability_registry()?;
        let build_identity = BuildIdentity::derive(
            "lumin-cli",
            "0.1.0",
            Some("test-rev"),
            registry.contract_digest(),
        );
        let first = query_binary_capabilities(&build_identity, &registry, None)?;
        assert_eq!(first.items.len(), 3);
        assert_eq!(first.scope_total, 4);
        assert!(first.next_query.is_some());

        let second =
            query_binary_capabilities(&build_identity, &registry, first.next_query.clone())?;
        assert_eq!(second.items.len(), 1);
        assert_eq!(second.scope_total, 4);
        assert!(second.next_query.is_none());
        Ok(())
    }

    #[test]
    fn different_build_identity_rejects_cursor() -> Result<(), Box<dyn std::error::Error>> {
        let registry = compiled_capability_registry()?;
        let identity_a = BuildIdentity::derive(
            "lumin-cli",
            "0.1.0",
            Some("rev-a"),
            registry.contract_digest(),
        );
        let identity_b = BuildIdentity::derive(
            "lumin-cli",
            "0.1.0",
            Some("rev-b"),
            registry.contract_digest(),
        );
        let first = query_binary_capabilities(&identity_a, &registry, None)?;
        let cursor = first
            .next_query
            .ok_or("expected next_query for page 1 of 4 items")?;

        // The cursor carries identity_a's scope, so validated_query rejects it
        // when the expected scope is identity_b.
        let result2 = query_binary_capabilities(&identity_b, &registry, Some(cursor));
        assert!(matches!(
            result2,
            Err(EngineError::EvidenceQuery(
                EvidenceQueryError::CursorScopeMismatch
            ))
        ));
        Ok(())
    }
}
