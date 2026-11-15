//! Node main entry points (port of `runtime/NodeMain.scala`).
//!
//! `run_cli` mirrors `NodeMain.runCLI` (the thin-client CLI); the `run` (start-node) path is wired
//! separately in `crate::main`.

use std::io::Read;
use std::path::Path;

use rchain_casper::protocol::client::{DeployRuntime, GrpcDeployService, GrpcProposeService, Name};
use rchain_crypto::private_key::PrivateKey;
use rchain_crypto::signatures::secp256k1::Secp256k1;
use rchain_crypto::signatures::signatures_alg::{from_algorithm, SignaturesAlg};
use rchain_crypto::util::key_util::write_keys;

use crate::configuration::commandline::options::{Commands, Options};
use crate::effects::{ConsoleIo, GrpcReplClient, RustylineConsole, StdioConsole};
use crate::runtime::recovery_verify;
use crate::runtime::repl_runtime::ReplRuntime;

/// The internal gRPC port used by the repl/propose clients by default (port of `Options.GrpcInternalPort`).
const GRPC_INTERNAL_PORT: i32 = 40402;
/// The external gRPC port used by the deploy client by default (port of `Options.GrpcExternalPort`).
const GRPC_EXTERNAL_PORT: i32 = 40401;

const VALIDATOR_PASSWORD_ENV_VAR: &str = "RNODE_VALIDATOR_PASSWORD";

/// Read all of stdin as text, for the two offline verifiers. Reading to EOF is the contract: the tool
/// pipes one node response in, and a truncated response is a refusal from the verifier rather than a
/// silent partial read.
fn read_stdin() -> Result<String, Vec<String>> {
    let mut text = String::new();
    std::io::stdin()
        .read_to_string(&mut text)
        .map_err(|e| vec![format!("could not read the response from stdin: {e}")])?;
    if text.trim().is_empty() {
        return Err(vec![
            "no response on stdin: pipe the node's JSON in (for example \
             `curl -s http://127.0.0.1:40403/api/v1/block/<hash>/raw | rnode verify-block`)"
                .to_string(),
        ]);
    }
    Ok(text)
}

/// Execute a CLI command against a remote node (port of `NodeMain.runCLI`).
pub async fn run_cli(options: &Options) -> Result<(), Vec<String>> {
    let host = options.grpc_host.clone();
    let max_size = options.grpc_max_recv_message_size;

    match &options.subcommand {
        Commands::Run(_) => Err(vec![
            "`run` is handled by the node, not the CLI client".to_string()
        ]),

        Commands::Keygen { location } => {
            let mut console = StdioConsole;
            generate_key(&mut console, location)
        }

        Commands::Repl => {
            let port = options.grpc_port.unwrap_or(GRPC_INTERNAL_PORT);
            let client = GrpcReplClient::connect(&host, port, max_size)
                .await
                .map_err(|e| vec![e])?;
            tokio::task::spawn_blocking(move || {
                let mut console = match RustylineConsole::new() {
                    Ok(c) => c,
                    Err(e) => {
                        eprintln!("Failed to initialize the REPL console: {e}");
                        return;
                    }
                };
                ReplRuntime.repl_program(&mut console, &client);
            })
            .await
            .map_err(|e| vec![e.to_string()])
        }

        Commands::Eval {
            file_names,
            print_unmatched_sends_only,
        } => {
            let port = options.grpc_port.unwrap_or(GRPC_INTERNAL_PORT);
            let client = GrpcReplClient::connect(&host, port, max_size)
                .await
                .map_err(|e| vec![e])?;
            let files = file_names.clone();
            let unmatched = *print_unmatched_sends_only;
            tokio::task::spawn_blocking(move || {
                let mut console = StdioConsole;
                ReplRuntime.eval_program(&mut console, &client, &files, unmatched);
            })
            .await
            .map_err(|e| vec![e.to_string()])
        }

        // The reconciliation tool's verifiers (AUDIT C261). **Offline, and before the deploy-service
        // arm below**: they read a node's JSON response on stdin and check it with this node's own
        // `hash_block` / `block_signature` / attestation-prehash, so nothing connects to anything.
        Commands::VerifyBlock => {
            let text = read_stdin()?;
            match recovery_verify::verify_block(&text) {
                Ok(verdict) => {
                    println!("{verdict}");
                    Ok(())
                }
                Err(reason) => Err(vec![reason]),
            }
        }
        // `audit` needs the node's genesis configuration (it builds a replay runtime), so it is
        // dispatched before the thin-client arm; see `main::async_main`.
        Commands::Audit { .. } => Err(vec![
            "`audit` is handled by the node, not the thin client".to_string()
        ]),
        Commands::VerifyAttestation { nonce } => {
            let text = read_stdin()?;
            match recovery_verify::verify_attestation(&text, nonce) {
                Ok(verdict) => {
                    println!("{verdict}");
                    Ok(())
                }
                Err(reason) => Err(vec![reason]),
            }
        }

        // The remaining subcommands talk to the deploy service (and the propose service for
        // `propose`).
        _ => {
            let external_port = options.grpc_port.unwrap_or(GRPC_EXTERNAL_PORT);
            let internal_port = options.grpc_port.unwrap_or(GRPC_INTERNAL_PORT);
            let deploy = GrpcDeployService::connect(&host, external_port, max_size)
                .await
                .map_err(|e| vec![e])?;

            match &options.subcommand {
                Commands::Propose {
                    print_unmatched_sends,
                } => {
                    let propose = GrpcProposeService::connect(&host, internal_port, max_size)
                        .await
                        .map_err(|e| vec![e])?;
                    DeployRuntime::propose(&propose, *print_unmatched_sends).await
                }
                Commands::Deploy {
                    phlo_limit,
                    phlo_price,
                    valid_after_block_number,
                    private_key,
                    private_key_path,
                    location,
                    shard_id,
                } => {
                    let mut console = StdioConsole;
                    let private_key = resolve_private_key(
                        private_key.as_deref(),
                        private_key_path.as_deref(),
                        &mut console,
                    )?;
                    DeployRuntime::deploy_file_program(
                        &deploy,
                        *phlo_limit,
                        *phlo_price,
                        valid_after_block_number.unwrap_or(-1),
                        &private_key,
                        location,
                        shard_id,
                    )
                    .await
                }
                Commands::DeployStatus { deploy_signature } => {
                    DeployRuntime::deploy_status(&deploy, &deploy_signature.0).await
                }
                Commands::FindDeploy { deploy_id } => {
                    DeployRuntime::find_deploy(&deploy, &deploy_id.0).await
                }
                Commands::ShowBlock { hash } => DeployRuntime::get_block(&deploy, hash).await,
                Commands::ShowBlocks { depth } => {
                    DeployRuntime::get_blocks(&deploy, depth.unwrap_or(1)).await
                }
                Commands::Vdag {
                    depth,
                    show_justification_lines,
                } => {
                    DeployRuntime::visualize_dag(
                        &deploy,
                        depth.unwrap_or(-1),
                        *show_justification_lines,
                    )
                    .await
                }
                Commands::Mvdag => DeployRuntime::machine_verifiable_dag(&deploy).await,
                Commands::ListenDataAtName {
                    type_of_name,
                    content,
                } => {
                    let name = build_single_name(type_of_name, content)?;
                    DeployRuntime::listen_for_data_at_name(&deploy, &name).await
                }
                Commands::ListenContAtName {
                    type_of_name,
                    content,
                } => {
                    let names = build_names(type_of_name, content)?;
                    DeployRuntime::listen_for_continuation_at_name(&deploy, &names).await
                }
                Commands::LastFinalizedBlock => DeployRuntime::last_finalized_block(&deploy).await,
                Commands::IsFinalized { hash } => DeployRuntime::is_finalized(&deploy, hash).await,
                Commands::BondStatus {
                    validator_public_key,
                } => DeployRuntime::bond_status(&deploy, &validator_public_key.0).await,
                Commands::Status => DeployRuntime::status(&deploy).await,
                _ => Err(vec!["unexpected subcommand for deploy service".to_string()]),
            }
        }
    }
}

/// Build a single listen-at-name `Name` from the `type`/`content` CLI args (port of the `SINGLE`
/// name converter in `Options.nameProviderConverter`).
fn build_single_name(type_of_name: &str, content: &[String]) -> Result<Name, Vec<String>> {
    let first = content.first().cloned().unwrap_or_default();
    match type_of_name {
        "priv" => Ok(Name::PrivName(first)),
        "pub" => Ok(Name::PubName(first)),
        _ => Err(vec!["Bad option value. Use \"pub\" or \"priv\"".to_string()]),
    }
}

/// Build a list of listen-at-name `Name`s (port of the `LIST` name converter).
fn build_names(type_of_name: &str, content: &[String]) -> Result<Vec<Name>, Vec<String>> {
    match type_of_name {
        "priv" => Ok(content.iter().map(|c| Name::PrivName(c.clone())).collect()),
        "pub" => Ok(content.iter().map(|c| Name::PubName(c.clone())).collect()),
        _ => Err(vec!["Bad option value. Use \"pub\" or \"priv\"".to_string()]),
    }
}

/// Resolve the deploy private key from `--private-key` (hex) or `--private-key-path` (PEM) (port of
/// `NodeMain.runCLI`'s `getPrivateKey`).
fn resolve_private_key(
    private_key: Option<&str>,
    private_key_path: Option<&Path>,
    console: &mut dyn ConsoleIo,
) -> Result<PrivateKey, Vec<String>> {
    if let Some(hex) = private_key {
        let bytes = rchain_shared::base16::decode(hex)
            .ok_or_else(|| vec!["Invalid base16 private key".to_string()])?;
        Ok(PrivateKey::new(bytes))
    } else if let Some(path) = private_key_path {
        decrypt_key_from_pem_file(console, path)
    } else {
        Err(vec!["Private key is missing".to_string()])
    }
}

/// Decrypt a PEM private key (port of `NodeMain.decryptKeyFromPemFile`).
fn decrypt_key_from_pem_file(
    console: &mut dyn ConsoleIo,
    path: &Path,
) -> Result<PrivateKey, Vec<String>> {
    let password = get_validator_password(console);
    Secp256k1::parse_pem_file(path, &password).map_err(|e| vec![e])
}

/// Read the validator password from the environment, falling back to the console (port of
/// `NodeMain.getValidatorPassword`).
fn get_validator_password(console: &mut dyn ConsoleIo) -> String {
    match std::env::var(VALIDATOR_PASSWORD_ENV_VAR) {
        Ok(password) if !password.is_empty() => password,
        _ => console.read_password(
            "Variable RNODE_VALIDATOR_PASSWORD is not set, please enter password for keyfile. \nPassword for keyfile: ",
        ),
    }
}

/// Generate a validator key pair and write it to `path` (port of `NodeMain.generateKey`).
///
/// An empty or mismatched password re-prompts by recursing, with no attempt limit — faithful to
/// Scala, and bounded in practice by the operator at the terminal (`spec/AUDIT.md` C7).
fn generate_key(console: &mut dyn ConsoleIo, path: &Path) -> Result<(), Vec<String>> {
    let password = console.read_password("Enter password for keyfile: ");
    let password_repeat = console.read_password("Repeat password: ");
    if password != password_repeat {
        console.println("Passwords do not match. Try again:");
        return generate_key(console, path);
    }
    if password.is_empty() {
        console.println("Password is empty. Try again:");
        return generate_key(console, path);
    }

    let sig_algorithm = from_algorithm(Secp256k1.name())
        .ok_or_else(|| vec!["Invalid algorithm name".to_string()])?;
    let (sk, pk) = sig_algorithm.new_key_pair();
    let private_path = path.join("rnode.key");
    let public_path = path.join("rnode.pub.pem");
    let hex_path = path.join("rnode.pub.hex");

    write_keys(
        &sk,
        &pk,
        sig_algorithm,
        &password,
        &private_path,
        &public_path,
        &hex_path,
    )
    .map_err(|e| vec![e])?;

    console.println(&format!(
        "\nSuccess!\nPrivate key file (encrypted PEM format):  {}\nPublic  key file (PEM format):            {}\nPublic  key file (HEX format):            {}",
        private_path.display(),
        public_path.display(),
        hex_path.display()
    ));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::path::PathBuf;

    /// A console that answers `read_password` from a script and records what was printed, so the
    /// retry loops can be driven without a terminal.
    #[derive(Default)]
    struct ScriptedConsole {
        passwords: std::cell::RefCell<VecDeque<String>>,
        printed: std::cell::RefCell<Vec<String>>,
    }

    impl ScriptedConsole {
        fn new(passwords: &[&str]) -> Self {
            ScriptedConsole {
                passwords: std::cell::RefCell::new(
                    passwords.iter().map(|s| s.to_string()).collect(),
                ),
                printed: std::cell::RefCell::new(Vec::new()),
            }
        }
        fn printed(&self) -> Vec<String> {
            self.printed.borrow().clone()
        }
    }

    impl ConsoleIo for ScriptedConsole {
        fn read_line(&mut self) -> Option<String> {
            None
        }
        fn read_password(&mut self, _prompt: &str) -> String {
            self.passwords.borrow_mut().pop_front().unwrap_or_default()
        }
        fn println(&mut self, s: &str) {
            self.printed.borrow_mut().push(s.to_string());
        }
        fn println_colored(&mut self, s: &rchain_shared::string_ops::ColoredString) {
            self.printed.borrow_mut().push(s.colorize());
        }
        fn update_completion(&mut self, _history: &[String]) {}
        fn close(&mut self) {}
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("rchain_node_main_{}_{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The `--listen-at-name` converter is one of the few CLI values that is *validated*, and the
    /// error must name both accepted spellings — an operator who typed `private` gets told `priv`.
    #[test]
    fn the_name_converter_accepts_only_pub_and_priv() {
        assert_eq!(
            build_single_name("priv", &["x".to_string()]).unwrap(),
            Name::PrivName("x".to_string())
        );
        assert_eq!(
            build_single_name("pub", &["x".to_string()]).unwrap(),
            Name::PubName("x".to_string())
        );
        // The `SINGLE` form takes the first element and tolerates an empty list.
        assert_eq!(
            build_single_name("priv", &[]).unwrap(),
            Name::PrivName(String::new())
        );

        for bad in ["Priv", "private", ""] {
            let err = build_single_name(bad, &["x".to_string()]).expect_err("let it error");
            assert_eq!(err.len(), 1);
            assert!(err[0].contains("\"pub\" or \"priv\""), "{err:?}");
        }
        // The `LIST` form maps every element, and rejects the same way.
        assert_eq!(
            build_names("pub", &["a".to_string(), "b".to_string()]).unwrap(),
            vec![
                Name::PubName("a".to_string()),
                Name::PubName("b".to_string())
            ]
        );
        assert!(build_names("nope", &["a".to_string()]).is_err());
        assert_eq!(build_names("priv", &[]).unwrap(), Vec::new());
    }

    /// The three arms of key resolution: an inline hex key wins, a path is decrypted through the
    /// console, and neither is an error rather than a panic (the node must be told to supply one).
    #[test]
    fn a_private_key_comes_from_hex_a_path_or_nowhere() {
        let mut console = ScriptedConsole::default();
        // A valid 32-byte scalar, hex-encoded.
        let hex = rchain_shared::base16::encode(&[7u8; 32]);
        let key = resolve_private_key(Some(&hex), None, &mut console).expect("hex key");
        assert_eq!(key.bytes(), &[7u8; 32]);

        // An inline key takes precedence even when a path is also given (the flag wins over the
        // default), so no file is touched and the console is not read.
        let key = resolve_private_key(Some(&hex), Some(Path::new("/nonexistent")), &mut console)
            .expect("hex key wins");
        assert_eq!(key.bytes(), &[7u8; 32]);
        assert!(console.printed().is_empty());

        let err = resolve_private_key(Some("not hex!"), None, &mut console).expect_err("bad hex");
        assert_eq!(err, vec!["Invalid base16 private key".to_string()]);

        let err = resolve_private_key(None, None, &mut console).expect_err("no key");
        assert_eq!(err, vec!["Private key is missing".to_string()]);

        // A path that does not exist surfaces the parse error rather than silently succeeding.
        let err = resolve_private_key(
            None,
            Some(Path::new("/nonexistent/rnode.key")),
            &mut console,
        )
        .expect_err("bad path");
        assert_eq!(err.len(), 1, "{err:?}");
    }

    /// `--generate-key` refuses an empty password and re-prompts on a mismatch, so a mistyped
    /// password cannot produce a key file nobody can decrypt. The retry is a real branch: the
    /// scripted console answers twice and the recursive call must consume both answers.
    #[test]
    fn generate_key_reprompts_on_a_mismatch_and_refuses_an_empty_password() {
        let dir = temp_dir("generate");
        // Mismatch, then a matching pair.
        let mut console = ScriptedConsole::new(&["first", "second", "agreed", "agreed"]);
        generate_key(&mut console, &dir).expect("eventually consistent passwords");

        let printed = console.printed();
        assert!(
            printed.iter().any(|p| p.contains("Passwords do not match")),
            "the mismatch must be reported: {printed:?}"
        );
        assert!(
            printed.iter().any(|p| p.contains("Success!")),
            "{printed:?}"
        );
        assert!(dir.join("rnode.key").exists());
        assert!(dir.join("rnode.pub.pem").exists());
        assert!(dir.join("rnode.pub.hex").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// **A PEM keyfile is unlocked from the environment** (W-N5-03).
    ///
    /// The row's class is key-at-rest, and its deviation is that the password guarding a key *on disk*
    /// has a non-interactive route **through the environment**: `RNODE_VALIDATOR_PASSWORD` is read
    /// before the console is ever consulted, so the secret that protects the keyfile at rest lives in
    /// the process environment for as long as the node runs — and, per W-N6-04, nothing clears it.
    ///
    /// Both directions, because either alone proves nothing: with the variable set the keyfile unlocks
    /// and the console is never prompted; with it unset and no terminal the same call fails. The second
    /// half is what keeps the first from being a fact about the console rather than about the route.
    #[test]
    fn the_env_var_unlocks_the_pem_keyfile_without_a_console() {
        let dir = temp_dir("env-unlock");
        let mut gen = ScriptedConsole::new(&["secret", "secret"]);
        generate_key(&mut gen, &dir).expect("keygen writes an encrypted keyfile");
        let key_path = dir.join("rnode.key");
        assert!(key_path.is_file(), "the fixture's keyfile");

        // (1) The environment is enough: no prompt reaches the console.
        std::env::set_var(VALIDATOR_PASSWORD_ENV_VAR, "secret");
        let mut console = ScriptedConsole::default();
        let key = resolve_private_key(None, Some(&key_path), &mut console)
            .expect("the environment unlocks the keyfile");
        assert_eq!(key.bytes().len(), 32, "a decrypted secp256k1 scalar");
        assert!(
            console.printed().is_empty(),
            "**the password was taken from the environment, so the console was never asked** \
             (W-N5-03): the non-interactive route exists, which is the deviation"
        );

        // (2) And without it, a console with nothing to give cannot open the file — so (1) is the
        //     environment's doing and not the keyfile being unprotected.
        std::env::remove_var(VALIDATOR_PASSWORD_ENV_VAR);
        let mut empty = ScriptedConsole::default();
        let err = resolve_private_key(None, Some(&key_path), &mut empty)
            .expect_err("no variable and no terminal: the keyfile does not open");
        assert!(
            err.join(" ").to_lowercase().contains("decrypt"),
            "and it fails for the password's reason rather than another: {err:?}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An empty password is refused with its own message, and the scripted console then supplies a
    /// real one — so "refuse and retry" is distinguished from "refuse and give up".
    #[test]
    fn generate_key_retries_after_an_empty_password() {
        let dir = temp_dir("empty");
        let mut console = ScriptedConsole::new(&["", "", "chosen", "chosen"]);
        generate_key(&mut console, &dir).expect("a real password arrives eventually");

        let printed = console.printed();
        assert!(
            printed.iter().any(|p| p.contains("Password is empty")),
            "{printed:?}"
        );
        assert!(
            printed.iter().any(|p| p.contains("Success!")),
            "{printed:?}"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A console that never supplies a usable password recurses until the stack runs out: the
    /// retry is `generate_key` calling itself with no attempt counter (Scala's `NodeMain.generateKey`
    /// does the same). Deliberately **not** a test — a stack-overflow probe aborts the test process
    /// and would pin nothing a reader could not see from the code; the shape is recorded in
    /// `spec/AUDIT.md` C7 instead. Interactively the retry is bounded by the operator, which is
    /// why the port keeps it.
    #[test]
    fn a_mismatch_message_names_the_retry_rather_than_the_mismatch_alone() {
        let dir = temp_dir("message");
        let mut console = ScriptedConsole::new(&["a", "b", "c", "c"]);
        generate_key(&mut console, &dir).expect("recovers");
        let printed = console.printed();
        // The prompt is re-issued on each attempt, so the operator sees why they are being asked
        // again — the message is the only feedback the recursion gives.
        assert_eq!(
            printed
                .iter()
                .filter(|p| p.contains("Passwords do not match"))
                .count(),
            1,
            "{printed:?}"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
