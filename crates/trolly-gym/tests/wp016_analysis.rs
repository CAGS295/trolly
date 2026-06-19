//! WP-016 acceptance: RL toolchain analysis doc exists and meets criteria.

use std::path::PathBuf;

fn analysis_doc_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("docs/ADR-001-rl-toolchain.md")
}

fn readme_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("README.md")
}

#[test]
fn wp016_analysis_doc_exists_and_covers_acceptance_criteria() {
    let path = analysis_doc_path();
    assert!(
        path.is_file(),
        "missing WP-016 analysis doc at {}",
        path.display()
    );

    let doc = std::fs::read_to_string(&path).expect("read analysis doc");

    let required_sections = [
        "tch / libtorch",
        "Candle",
        "Burn",
        "ONNX Runtime",
        "Python / PyTorch sidecar",
        "Integration surface",
        "Offline training vs online inference",
        "Decision",
        "Follow-on work items",
    ];
    for section in required_sections {
        assert!(
            doc.contains(section),
            "analysis doc missing required section or candidate: {section}"
        );
    }

    let integration_points = [
        "Env",
        "ObservationWindow",
        "ReplayBuffer",
        "Action",
        "StreamEgress",
        "backpressure",
    ];
    for point in integration_points {
        assert!(
            doc.contains(point),
            "analysis doc missing integration point: {point}"
        );
    }

    let rl_families = ["PPO", "DQN", "SAC", "offline"];
    for family in rl_families {
        assert!(
            doc.contains(family),
            "analysis doc missing RL algorithm coverage: {family}"
        );
    }

    let ops_topics = ["GPU", "CI", "libtorch", "hot-swap", "fallback"];
    for topic in ops_topics {
        assert!(
            doc.contains(topic),
            "analysis doc missing ops topic: {topic}"
        );
    }

    assert!(
        doc.contains("Primary toolchain") && doc.contains("Optional fallback"),
        "analysis doc must record explicit primary and fallback decisions"
    );

    for wp in ["WP-018", "WP-019", "WP-020"] {
        assert!(
            doc.contains(wp),
            "analysis doc must list follow-on WP: {wp}"
        );
    }
}

#[test]
fn wp016_readme_links_analysis_doc() {
    let readme = std::fs::read_to_string(readme_path()).expect("read README");
    assert!(
        readme.contains("docs/ADR-001-rl-toolchain.md"),
        "README must link to WP-016 analysis doc"
    );
    assert!(
        readme.contains("WP-016"),
        "README must reference WP-016"
    );
}
