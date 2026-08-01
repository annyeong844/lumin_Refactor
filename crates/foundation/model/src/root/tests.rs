use super::*;

const ROOT_VECTORS: [&str; 4] = [
    "4c554d52524f4f54000101010000000101000000047265706f0100000000000000010000000000000002",
    "4c554d52524f4f5400010202430000000101000000047265706f020000000000000001000102030405060708090a0b0c0d0e0f",
    "4c554d52524f4f54000102030100000006736572766572010000000573686172650000000101000000047265706f020000000000000001000102030405060708090a0b0c0d0e0f",
    "4c554d52524f4f540001020400112233445566778899aabbccddeeff0000000101000000047265706f020000000000000001000102030405060708090a0b0c0d0e0f",
];

#[test]
fn decodes_every_frozen_root_vector() -> Result<(), Box<dyn std::error::Error>> {
    let expected_display = [
        "/repo",
        "C:/repo",
        "//server/share/repo",
        "//?/Volume{00112233-4455-6677-8899-aabbccddeeff}/repo",
    ];
    let mut repository_ids = std::collections::BTreeSet::new();
    for (vector, display) in ROOT_VECTORS.into_iter().zip(expected_display) {
        let bytes = decode_hex(vector)?;
        let root = RepositoryRootIdentity::from_canonical_bytes(&bytes)?;
        assert_eq!(root.canonical_bytes(), bytes);
        assert_eq!(root.display_escaped(), display);
        assert_eq!(root.readable_address().as_deref(), Some(display));
        repository_ids.insert(RepositoryId::for_root(&root));
    }
    assert_eq!(repository_ids.len(), ROOT_VECTORS.len());
    Ok(())
}

#[test]
fn rejects_noncanonical_drive_and_trailing_bytes() -> Result<(), Box<dyn std::error::Error>> {
    let mut lowercase_drive = decode_hex(ROOT_VECTORS[1])?;
    lowercase_drive[12] = b'c';
    assert_eq!(
        RepositoryRootIdentity::from_canonical_bytes(&lowercase_drive),
        Err(RepositoryRootError::InvalidCanonicalEncoding)
    );

    let mut trailing = decode_hex(ROOT_VECTORS[0])?;
    trailing.push(0);
    assert_eq!(
        RepositoryRootIdentity::from_canonical_bytes(&trailing),
        Err(RepositoryRootError::InvalidCanonicalEncoding)
    );

    let mut physical_platform_mismatch = decode_hex(ROOT_VECTORS[0])?;
    let physical_tag = physical_platform_mismatch.len() - 17;
    physical_platform_mismatch[physical_tag] = WINDOWS_PHYSICAL_IDENTITY_TAG;
    assert_eq!(
        RepositoryRootIdentity::from_canonical_bytes(&physical_platform_mismatch),
        Err(RepositoryRootError::InvalidCanonicalEncoding)
    );
    Ok(())
}

fn decode_hex(value: &str) -> Result<Vec<u8>, std::num::ParseIntError> {
    (0..value.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&value[index..index + 2], 16))
        .collect()
}
