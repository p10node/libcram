//! NUMA topology. CXL transport alone never establishes compression capability.
mod numa;
use crate::Error;
pub use numa::*;
use std::collections::BTreeSet;
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompressionProvenance {
    OperatorConfigured,
    HardwareVerified,
    /// Explicit driver adapter flag, not independent hardware validation.
    DriverReported,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Node {
    pub id: u32,
    pub is_cxl: bool,
    pub compression: Option<CompressionProvenance>,
}
#[derive(Clone, Debug)]
pub struct Topology {
    pub nodes: Vec<Node>,
}
impl Topology {
    pub fn from_nodes(nodes: Vec<Node>) -> Result<Self, Error> {
        let mut ids = BTreeSet::new();
        if nodes.is_empty() || nodes.iter().any(|n| n.id > 65535 || !ids.insert(n.id)) {
            return Err(Error::InvalidTopology);
        }
        Ok(Self { nodes })
    }
    pub fn cram_node(&self) -> Option<u32> {
        self.nodes
            .iter()
            .find(|n| n.compression.is_some())
            .map(|n| n.id)
    }
    pub fn dram_node(&self) -> Option<u32> {
        self.nodes
            .iter()
            .find(|n| !n.is_cxl && n.compression.is_none())
            .map(|n| n.id)
    }
    pub fn discover() -> Result<Self, Error> {
        if cfg!(target_os = "linux") {
            Self::discover_at(
                Path::new("/sys/devices/system/node"),
                Path::new("/sys/bus/cxl/devices"),
            )
        } else {
            Self::from_nodes(vec![Node {
                id: 0,
                is_cxl: false,
                compression: None,
            }])
        }
    }
    /// Read-only discovery; deliberately does not guess compression from CXL membership.
    pub fn discover_at(nodes: &Path, cxl: &Path) -> Result<Self, Error> {
        let mut cxl_nodes = BTreeSet::new();
        if let Ok(entries) = std::fs::read_dir(cxl) {
            for entry in entries.flatten() {
                if let Ok(text) = std::fs::read_to_string(entry.path().join("numa_node")) {
                    if let Ok(id) = text.trim().parse::<u32>() {
                        cxl_nodes.insert(id);
                    }
                }
            }
        }
        let mut found = Vec::new();
        let entries = match std::fs::read_dir(nodes) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                // UMA kernels and sandboxed containers may not expose NUMA sysfs.
                return Self::from_nodes(vec![Node {
                    id: 0,
                    is_cxl: false,
                    compression: None,
                }]);
            }
            Err(error) => return Err(error.into()),
        };
        for entry in entries {
            let entry = entry?;
            if let Some(id) = entry
                .file_name()
                .to_str()
                .and_then(|s| s.strip_prefix("node"))
                .and_then(|s| s.parse::<u32>().ok())
            {
                found.push(Node {
                    id,
                    is_cxl: cxl_nodes.contains(&id),
                    // Optional libcram driver-adapter convention, not a generic CXL ABI.
                    // Missing, disabled and malformed flags all remain uncompressed.
                    compression: std::fs::read_to_string(
                        entry.path().join("cram/compression_enabled"),
                    )
                    .ok()
                    .filter(|text| text.trim() == "1")
                    .map(|_| CompressionProvenance::DriverReported),
                });
            }
        }
        found.sort_by_key(|n| n.id);
        Self::from_nodes(found)
    }
}
