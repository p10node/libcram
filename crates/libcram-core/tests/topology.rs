use libcram_core::hal::{CompressionProvenance, Node, Topology};
#[test]
fn cxl_is_not_compressed_without_explicit_provenance() {
    let t = Topology::from_nodes(vec![
        Node {
            id: 0,
            is_cxl: false,
            compression: None,
        },
        Node {
            id: 2,
            is_cxl: true,
            compression: None,
        },
    ])
    .expect("topology");
    assert_eq!(t.cram_node(), None);
    let t = Topology::from_nodes(vec![
        Node {
            id: 0,
            is_cxl: false,
            compression: None,
        },
        Node {
            id: 2,
            is_cxl: true,
            compression: Some(CompressionProvenance::OperatorConfigured),
        },
    ])
    .expect("topology");
    assert_eq!(t.cram_node(), Some(2));
    assert_eq!(t.dram_node(), Some(0));
    assert!(Topology::from_nodes(vec![
        Node {
            id: 0,
            is_cxl: false,
            compression: None
        };
        2
    ])
    .is_err());
}

#[test]
fn unavailable_sysfs_falls_back_to_uncompressed_uma() {
    let missing = std::path::Path::new("/nonexistent-libcram-fixture/node");
    let t = Topology::discover_at(missing, missing).expect("UMA fallback");
    assert_eq!(t.cram_node(), None);
    assert_eq!(t.dram_node(), Some(0));
}

#[test]
fn mock_sysfs_discovers_cxl_but_requires_compression_provenance() {
    let base = std::env::temp_dir()
        .join("libcram-topology-fixture")
        .join(std::process::id().to_string());
    let nodes = base.join("node");
    let cxl = base.join("cxl");
    std::fs::create_dir_all(nodes.join("node0")).expect("node0");
    std::fs::create_dir_all(nodes.join("node4")).expect("node4");
    std::fs::create_dir_all(cxl.join("mem0")).expect("mem0");
    std::fs::write(cxl.join("mem0/numa_node"), "4\n").expect("numa_node");
    let t = Topology::discover_at(&nodes, &cxl).expect("discovery");
    assert_eq!(t.nodes.len(), 2);
    assert!(t.nodes[1].is_cxl);
    assert_eq!(t.cram_node(), None);
    std::fs::remove_dir_all(base).expect("cleanup");
}

#[test]
fn explicit_driver_flag_has_enabled_disabled_and_malformed_controls() {
    let base = std::env::temp_dir().join(format!("libcram-flags-{}", std::process::id()));
    let nodes = base.join("node");
    std::fs::create_dir_all(nodes.join("node0")).expect("dram");
    std::fs::create_dir_all(nodes.join("node7/cram")).expect("adapter");
    let flag = nodes.join("node7/cram/compression_enabled");
    for (value, expected) in [("1\n", Some(7)), ("0\n", None), ("yes", None)] {
        std::fs::write(&flag, value).expect("fixture flag");
        assert_eq!(
            Topology::discover_at(&nodes, &base.join("absent-cxl"))
                .expect("topology")
                .cram_node(),
            expected
        );
    }
    std::fs::remove_dir_all(base).expect("owned fixture cleanup");
}
