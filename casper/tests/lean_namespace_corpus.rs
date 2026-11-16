//! Law 56's conformance corpus: the `namespace` layer.
//!
//! The consumer drives the deployed `MasterDictionary.rho` through the same probe harness the rule's
//! own tests use (`common::dictionary`), one row per verdict the Lean model states.

mod common;

use common::dictionary::*;

/// Law 56's conformance corpus: the `namespace` layer.
///
/// Each row is one of the rooted-namespace verdicts the Lean model states. The consumer builds the
/// same probe the law's own tests use and drives the deployed `MasterDictionary.rho`; a row whose
/// verdict drifts from the model fails here.
#[test]
fn lean_namespace_corpus() {
    with_big_stack(async {
        let (p, a) = (publish_uri(), root_uri());
        let corpus = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../spec/conformance/namespace.tsv");
        let text = std::fs::read_to_string(&corpus).unwrap_or_else(|e| {
            panic!(
                "read {}: {e}\n(run tools/emit-lean-corpus.sh)",
                corpus.display()
            )
        });
        let mut probes: Vec<(String, u8)> = Vec::new();
        let mut rows = 0;
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            let cols: Vec<&str> = line.split('\t').collect();
            assert_eq!(
                cols.len(),
                7,
                "a namespace row is kind/verb/caller/owner/name/sealed/expected: {line:?}"
            );
            assert_eq!(cols[0], "namespace", "{line:?}");
            let caller: u8 = cols[2].parse().expect("caller");
            let owner: u8 = cols[3].parse().expect("owner");
            let name: u64 = cols[4].parse().expect("name");
            let sealed: bool = cols[5].parse().expect("sealed");
            match cols[1] {
                "publish" => {
                    let path = format!("{}/inbox-{name}", rev_address_of(owner));
                    if sealed {
                        probes.push((
                            probe(
                                &p,
                                "publish",
                                &format!(r#"[*deployerId, "{path}", "v0"]"#),
                                true,
                                &format!(r#"("published", "{path}", 0)"#),
                            ),
                            caller,
                        ));
                        probes.push((
                            probe(
                                &p,
                                "seal",
                                &format!(r#"[*deployerId, "{path}"]"#),
                                true,
                                &format!(r#"("sealed", "{path}")"#),
                            ),
                            caller,
                        ));
                    }
                    let expected = match cols[6] {
                        "published" => format!(r#"("published", "{path}", 0)"#),
                        "not-your-namespace" => {
                            r#"("dir-error", "not your namespace")"#.to_string()
                        }
                        "sealed" => r#"("dir-error", "sealed")"#.to_string(),
                        other => panic!("no publish verdict {other:?}"),
                    };
                    probes.push((
                        probe(
                            &p,
                            "publish",
                            &format!(r#"[*deployerId, "{path}", "v1"]"#),
                            true,
                            &expected,
                        ),
                        caller,
                    ));
                }
                "alias" => {
                    let expected = match cols[6] {
                        "not-root-authority" => {
                            r#"("dir-error", "not the root authority")"#.to_string()
                        }
                        other => panic!("no alias verdict {other:?}"),
                    };
                    probes.push((
                        probe(
                            &a,
                            "alias",
                            r#"[*deployerId, "Inbox", "rho:id:evil"]"#,
                            true,
                            &expected,
                        ),
                        caller,
                    ));
                }
                other => panic!("no namespace verb {other:?}"),
            }
            rows += 1;
        }
        let tags = run(probes).await;
        assert_eq!(
            tags.len(),
            6,
            "one probe per publish and three for the sealed row"
        );
        assert!(tags.iter().all(|t| t == "ok"), "tags: {tags:?}");
        assert_eq!(rows, 4, "the corpus carried four rows");
    });
}
