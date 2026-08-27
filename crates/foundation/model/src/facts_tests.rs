use crate::{
    SOURCE_CLASSIFICATION_RULE_VERSION, SourceClassificationRole, SourceRoleClassification,
    SourceRoleConfigurationSource, SourceRoleReason, SourceRoles, external_package_name,
    validate_scan_pattern,
};

#[test]
fn external_package_names_use_the_owner_projection() {
    assert_eq!(external_package_name("react/jsx-runtime"), "react");
    assert_eq!(external_package_name("@scope/pkg/subpath"), "@scope/pkg");
    assert_eq!(external_package_name("#internal"), "#internal");
}

#[test]
fn scan_pattern_validation_matches_the_inventory_admission_grammar() {
    for pattern in [
        "src/**",
        "**/*.ts",
        "[a-z]/**",
        "[z-a",
        "{src,test}/**",
        r"\!literal",
    ] {
        assert!(validate_scan_pattern(pattern).is_ok(), "{pattern}");
    }
    for pattern in [
        "",
        "!src/**",
        "../src/**",
        r"src\",
        "[z-a]",
        "{src,test",
        "src}",
    ] {
        assert!(validate_scan_pattern(pattern).is_err(), "{pattern}");
    }
}

fn classification(
    role: SourceClassificationRole,
    reason: SourceRoleReason,
    configuration_source: SourceRoleConfigurationSource,
) -> SourceRoleClassification {
    SourceRoleClassification {
        role,
        rule_version: SOURCE_CLASSIFICATION_RULE_VERSION.to_owned(),
        reason,
        configuration_source,
    }
}

#[test]
fn effective_roles_are_derived_from_ordered_classifications() {
    let classifications = vec![
        classification(
            SourceClassificationRole::Test,
            SourceRoleReason::TestPathRule,
            SourceRoleConfigurationSource::CompiledDefault,
        ),
        classification(
            SourceClassificationRole::Generated,
            SourceRoleReason::LeadingGeneratedComment,
            SourceRoleConfigurationSource::CompiledDefault,
        ),
        classification(
            SourceClassificationRole::Authored,
            SourceRoleReason::ExplicitAuthoredRole,
            SourceRoleConfigurationSource::Configuration,
        ),
        classification(
            SourceClassificationRole::Production,
            SourceRoleReason::ExplicitProductionRole,
            SourceRoleConfigurationSource::Invocation,
        ),
        classification(
            SourceClassificationRole::Vendor,
            SourceRoleReason::ExplicitVendorRole,
            SourceRoleConfigurationSource::Invocation,
        ),
    ];
    let roles = SourceRoles::from_classifications(classifications.clone());

    assert_eq!(roles.classifications(), classifications);
    assert_eq!(roles.test_like_reason(), None);
    assert_eq!(roles.generated_reason(), None);
    assert_eq!(
        roles.vendored_reason(),
        Some(SourceRoleReason::ExplicitVendorRole)
    );
    assert!(!roles.is_test_like());
    assert!(!roles.is_generated());
    assert!(roles.is_vendored());
    assert!(!roles.is_declaration());
}

#[test]
fn higher_tier_classification_can_restore_a_cleared_role() {
    let roles = SourceRoles::from_classifications(vec![
        classification(
            SourceClassificationRole::Generated,
            SourceRoleReason::LeadingGeneratedComment,
            SourceRoleConfigurationSource::CompiledDefault,
        ),
        classification(
            SourceClassificationRole::Authored,
            SourceRoleReason::ExplicitAuthoredRole,
            SourceRoleConfigurationSource::Configuration,
        ),
        classification(
            SourceClassificationRole::Generated,
            SourceRoleReason::ExplicitGeneratedRole,
            SourceRoleConfigurationSource::Invocation,
        ),
        classification(
            SourceClassificationRole::Declaration,
            SourceRoleReason::DeclarationExtension,
            SourceRoleConfigurationSource::CompiledDefault,
        ),
    ]);

    assert_eq!(
        roles.generated_reason(),
        Some(SourceRoleReason::ExplicitGeneratedRole)
    );
    assert!(roles.is_generated());
    assert!(roles.is_declaration());
}
