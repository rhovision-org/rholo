# Public testnet deployment snapshot — 2026-10-29

[snapshot.json](snapshot.json) is a read-only sample taken through the existing authenticated
`rholo-testnet-a` SSH connection, with strict host-key verification. It queries status, the
finalized block and block zero at all four previously identified validator endpoints. It also hashes
host A's installed executable and its public genesis input files, reads memory/CPU/disk information,
and records listener and unit state. No private key or environment credential file is read.

The reported host B capacity and the wallet-account names are attributed to the operator records:

- [Resize](https://github.com/rhovision-org/rholo/issues/479#issuecomment-6492321348).
- [Rebuild and funded accounts](https://github.com/rhovision-org/rholo/issues/479#issuecomment-6497437735).
- [Deployment documentation corrections](https://github.com/rhovision-org/rholo/issues/479#issuecomment-6497829943).

The snapshot distinguishes observations from operator reports. All four API versions agree on the
source revision and genesis. The binary checksum is independently measured on A; the operator's
record is the source for the matching installed checksum on B. Public input file checksums and
resource/listener readings in this snapshot are A's, not an independent inspection of B's filesystem.

This sample is a small, freshly rebuilt chain. Matching finalized hashes and responsive APIs do not
prove mature-chain fresh join, restart under load, application conformance, full hardware sizing or
recovery of the preserved old chain. The old `c596bef4…` evidence remains historical and unchanged.

No validator binary, service configuration, chain store, identity, Genesis or firewall rule was
modified by this collection. No observer or validator was started and no transaction was submitted.

To independently repeat the public readings, run these GETs on host A through an authenticated SSH
session with strict host-key verification:

```sh
for base in http://127.0.0.1:40403 http://127.0.0.1:41403 http://167.99.43.214:40403 http://167.99.43.214:41403; do
  for route in /api/status /api/last-finalized-block /api/blocks/0/0; do
    curl --fail --silent --show-error --max-time 8 "$base$route"
  done
done
sha256sum /usr/local/bin/rnode-d1af45d1
sudo -n sha256sum /var/lib/rnode/genesis/bonds.txt /var/lib/rnode/genesis/wallets.txt
systemctl show rnode rnode-d -p MainPID -p NRestarts -p ActiveState -p ExecMainStartTimestamp
ss -lntu
```

Height and peer readings can change. Verify the expected genesis and binary against the trusted
operator record; do not silently replace an operator's trusted hash with a new endpoint answer.

The exact structured collector is [collect_snapshot.py](collect_snapshot.py). From the repository
root, it can be streamed to the trusted host without writing a script onto that host:

```sh
ssh -T -o BatchMode=yes -o StrictHostKeyChecking=yes rholo-testnet-a python3 -B - < spec/audit/evidence/n479-current-epoch-20261029/collect_snapshot.py
```
