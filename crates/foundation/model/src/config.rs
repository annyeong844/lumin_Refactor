use std::collections::BTreeMap;

use crate::{LogicalSourceId, RepoPath};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq)]
pub struct ConfigDocument {
    pub path: RepoPath,
    pub payload_sha256: String,
    pub root: ConfigValue,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ConfigValue {
    Null,
    Boolean(bool),
    Number(String),
    String(String),
    Array(Vec<ConfigValue>),
    Object(Vec<ConfigEntry>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct ConfigEntry {
    pub key: String,
    pub value: ConfigValue,
}

impl ConfigValue {
    pub fn get(&self, key: &str) -> Option<&Self> {
        self.as_object()?
            .iter()
            .find(|entry| entry.key == key)
            .map(|entry| &entry.value)
    }

    pub fn as_object(&self) -> Option<&[ConfigEntry]> {
        match self {
            Self::Object(entries) => Some(entries),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Self]> {
        match self {
            Self::Array(values) => Some(values),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(value) => Some(value),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ConfigSyntax {
    StrictJson,
    Jsonc,
    RestrictedYaml,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ConfigObservation {
    Present(ConfigDocument),
    Missing { path: RepoPath },
    NonRegular { path: RepoPath },
    Unreadable { path: RepoPath, detail: String },
}

impl ConfigObservation {
    pub fn path(&self) -> &RepoPath {
        match self {
            Self::Present(document) => &document.path,
            Self::Missing { path } | Self::NonRegular { path } | Self::Unreadable { path, .. } => {
                path
            }
        }
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct PackageIdentity(String);

impl PackageIdentity {
    pub fn new(value: String) -> Self {
        Self(value)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PackageIdentityState {
    Missing,
    Valid(PackageIdentity),
    Unsupported,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PackagePrivacy {
    Unspecified,
    Public,
    Private,
    Unsupported,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PackageFact {
    pub root: RepoPath,
    pub manifest_path: RepoPath,
    pub identity: PackageIdentityState,
    pub privacy: PackagePrivacy,
    pub workspace_root: Option<RepoPath>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkspaceSource {
    PackageJson,
    PnpmWorkspace,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkspaceFact {
    pub root: RepoPath,
    pub source: WorkspaceSource,
    pub members: Vec<RepoPath>,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum PackageSurfaceLane {
    BundlerImport,
    LegacyNode,
    NodeImport,
    NodeRequire,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum PackageSurfaceSource {
    Exports {
        key: String,
        condition: Option<String>,
        lane: PackageSurfaceLane,
    },
    Module {
        lane: PackageSurfaceLane,
    },
    Main {
        lane: PackageSurfaceLane,
    },
    Typings {
        lane: PackageSurfaceLane,
    },
    Types {
        lane: PackageSurfaceLane,
    },
    DeclarationCompanion {
        lane: PackageSurfaceLane,
    },
    DirectoryIndex {
        lane: PackageSurfaceLane,
    },
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct PackageSurfaceDeclaration {
    pub package_root: RepoPath,
    pub manifest_path: RepoPath,
    pub request: String,
    pub namespace: crate::SymbolNamespace,
    pub source: PackageSurfaceSource,
    pub target: LogicalSourceId,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SemanticConfigSnapshot {
    pub observations: BTreeMap<RepoPath, ConfigObservation>,
    pub packages: Vec<PackageFact>,
    pub workspaces: Vec<WorkspaceFact>,
    pub source_packages: BTreeMap<LogicalSourceId, RepoPath>,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ResolutionProfile {
    Bundler,
    Node,
    Node16,
    NodeNext,
}

impl ResolutionProfile {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Bundler => "bundler",
            Self::Node => "node",
            Self::Node16 => "node16",
            Self::NodeNext => "nodenext",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ResolutionProfileSource {
    Invocation,
    Config {
        path_canonical: Vec<u8>,
        path_display: String,
    },
    ProductDefault,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectedResolutionProfile {
    pub source_id: LogicalSourceId,
    pub profile: ResolutionProfile,
    pub source: ResolutionProfileSource,
}
