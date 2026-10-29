import hashlib,json,re,gzip
from pathlib import Path
b=Path(__file__).resolve().parent
manifest=json.loads((b/"fixture-manifest.json").read_text())
compressed=(b/"block-metadata.dump.gz").read_bytes()
assert hashlib.sha256(compressed).hexdigest()==manifest["compressed_sha256"]
assert hashlib.sha256(gzip.decompress(compressed)).hexdigest()==manifest["uncompressed_sha256"]
preserved=json.loads((b/"source-preservation.json").read_text())
assert preserved["all_source_database_hashes_unchanged"]
setup=json.loads((b/"candidate-setup.json").read_text())
assert preserved["source_database_sha256"]==setup["source_database_sha256"]
red=(b/"baseline-3122.filtered.log").read_text()
assert "Self-created block #3122" in red and "a justification regressed" in red
for name in ["candidate-3122","candidate-restart"]:
 text=(b/(name+".filtered.log")).read_text()
 assert "a justification regressed" not in text
green=(b/"candidate-3122.filtered.log").read_text()
assert "proposed and added block #3122 (seq 3122)" in green
restart=(b/"candidate-restart.filtered.log").read_text()
for height in [3128,3129,3130,3131]:assert f"proposed and added block #{height} (seq {height})" in restart
rows=json.loads((b/"readback-acceptance.json").read_text())["cases"]
assert len(rows)==2 and all("error" not in r for r in rows)
expected_genesis="c596bef4bb678b3ac0cdb931b5b6968653209ca40bc35b6fec0050254d479b26"
finalized="ec5a08ef2307dc9a4e44a75e3f123ac345776a7c8b7a7811859060a78021f941"
for r in rows:
 assert r["isolation"]=="PrivateNetwork=yes"
 assert r["net_namespace"]!=r["host_net_namespace"]
 assert [x["blockHash"] for x in r["genesis"]]==[expected_genesis]
 assert r["finalized"]["blockInfo"]["blockHash"]==finalized
 assert r["finalized"]["blockInfo"]["blockNumber"]==1759
 assert r["deploy_status"]["ProcessedWithSuccess"]["block"]["blockNumber"]==3129
 assert r["deploy_status"]["ProcessedWithSuccess"]["block"]["blockHash"]=="652f47a1216c23786230b2f0f209860bcba1a73a3d410594f211f9c31e5ae062"
 assert r["status"]["latestBlockNumber"]==3132
assert rows[0]["blocks"]==rows[1]["blocks"]
assert rows[0]["deploy_status"]==rows[1]["deploy_status"]
before=json.loads((b/"restart-observations.json").read_text())["cases"]
assert all(r["finalized"]["blockInfo"]["blockHash"]==finalized for r in before)
print(json.dumps({"native_original_rejection":True,"candidate_accepts_3122":True,"restart_accepts_3128_through_3131":True,"epoch_boundary_3130":True,"signed_deploy_processed":True,"old_reader_same_blocks_and_deploy":True,"finalized_history_unchanged":True,"source_stores_unchanged":True,"scope":"Isolated historical-store experiment; not live consensus recovery, target-finality certification or hardware power-loss qualification"}))
