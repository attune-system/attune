use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub schema: Schema,
    pub pack: PackIdentity,
    pub requires: Requirements,
    pub dependencies: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<Source>,
    pub files: Vec<FileEntry>,
    pub artifacts: Vec<Artifact>,
    pub evidence: Evidence,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum Schema {
    #[serde(rename = "attune.pack.release/v1")]
    V1,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PackIdentity {
    pub r#ref: String,
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Requirements {
    pub attune: String,
    pub platform_catalog: String,
    pub runtimes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub repository: String,
    pub revision: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FileEntry {
    pub path: String,
    pub size: u64,
    pub sha256: String,
    pub mode: FileMode,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum FileMode {
    #[serde(rename = "0644")]
    Data,
    #[serde(rename = "0755")]
    Executable,
}

impl FileMode {
    pub(super) fn octal(self) -> u64 {
        match self {
            Self::Data => 0o644,
            Self::Executable => 0o755,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Artifact {
    Native {
        id: String,
        component_refs: Vec<String>,
        variants: Vec<NativeVariant>,
    },
    Jar {
        id: String,
        component_refs: Vec<String>,
        variants: Vec<JarVariant>,
    },
}

impl Artifact {
    pub fn id(&self) -> &str {
        match self {
            Self::Native { id, .. } | Self::Jar { id, .. } => id,
        }
    }

    pub fn component_refs(&self) -> &[String] {
        match self {
            Self::Native { component_refs, .. } | Self::Jar { component_refs, .. } => {
                component_refs
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct NativeVariant {
    pub id: String,
    pub path: String,
    pub target: NativeTarget,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(deny_unknown_fields)]
pub struct NativeTarget {
    pub os: NativeOs,
    pub arch: NativeArch,
    pub libc: NativeLibc,
}

impl NativeTarget {
    pub fn variant_id(&self) -> String {
        format!(
            "linux-{}-static",
            match self.arch {
                NativeArch::Amd64 => "amd64",
                NativeArch::Arm64 => "arm64",
            }
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum NativeOs {
    Linux,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum NativeArch {
    Amd64,
    Arm64,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum NativeLibc {
    Static,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct JarVariant {
    pub id: String,
    pub path: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Evidence {
    pub sboms: Vec<EvidenceFile>,
    pub provenance: Vec<EvidenceFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct EvidenceFile {
    pub path: String,
    pub media_type: String,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum LaunchSpec {
    File {
        path: String,
    },
    Native {
        artifact: String,
    },
    JavaJar {
        artifact: String,
        #[serde(default)]
        jvm_args: Vec<String>,
    },
    JavaClass {
        main_class: String,
        classpath: Vec<ClasspathEntry>,
        #[serde(default)]
        jvm_args: Vec<String>,
    },
    Intrinsic {
        handler: IntrinsicHandler,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum IntrinsicHandler {
    #[serde(rename = "attune.inquiry/v1")]
    InquiryV1,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(untagged, deny_unknown_fields)]
pub enum ClasspathEntry {
    Artifact { artifact: String },
    Path { path: String },
}
