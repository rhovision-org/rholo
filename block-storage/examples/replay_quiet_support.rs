//! Replay archived DAG inputs through the actual finalizer. Input is tab-separated:
//! hash, height, sequence, sender, comma-separated parents, comma-separated sender:stake bonds.
use rchain_block_storage::dag::{
    finalizer::{Finalizer, Message},
    liveness,
    message_state::DagMessageState,
};
use rchain_shared::refined::{BlockHeight, NonNegI64, SeqNum};
use std::collections::{BTreeMap, BTreeSet};

fn main() {
    let path = std::env::args().nth(1).expect("DAG TSV path");
    let input = std::fs::read_to_string(path).expect("read DAG TSV");
    let mut rows: Vec<_> = input
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| {
            let c: Vec<_> = l.split('\t').map(str::to_owned).collect();
            assert_eq!(c.len(), 6);
            c
        })
        .collect();
    rows.sort_by_key(|c| (c[1].parse::<i64>().unwrap(), c[0].clone()));
    let mut state: DagMessageState<String, String> = DagMessageState::empty();
    for c in rows {
        let parents: BTreeSet<Message<String, String>> = c[4]
            .split(',')
            .filter(|x| !x.is_empty())
            .map(|id| {
                state
                    .msg_map
                    .get(id)
                    .expect("missing causal parent")
                    .clone()
            })
            .collect();
        let bonds: BTreeMap<String, NonNegI64> = c[5]
            .split(',')
            .map(|v| {
                let (sender, stake) = v.split_once(':').unwrap();
                (
                    sender.to_owned(),
                    NonNegI64::try_from(stake.parse::<i64>().unwrap()).unwrap(),
                )
            })
            .collect();
        let height = BlockHeight::try_from(c[1].parse::<i64>().unwrap()).unwrap();
        let finalizer = Finalizer::new(&state.msg_map);
        let (prev, _, reason) =
            liveness::calculate_finalization_detailed(&finalizer, &parents, &bonds);
        if let Some(rchain_block_storage::dag::finalizer::NoAdvance::Support {
            supporting,
            total,
            ..
        }) = reason
        {
            let latest =
                liveness::latest_heights(parents.iter().map(|m| (m.sender.clone(), m.height)));
            let tip = liveness::quorum_supported_height(&bonds, &latest);
            let live = liveness::live_weight_set(&bonds, &latest, tip, liveness::LIVENESS_WINDOW);
            let js: BTreeSet<_> = parents
                .iter()
                .filter(|m| live.contains_key(&m.sender))
                .cloned()
                .collect();
            let mut minimum = Vec::new();
            for j in &js {
                let mut m = j.clone();
                while let Some(p) = m
                    .parents
                    .iter()
                    .filter_map(|h| state.msg_map.get(h))
                    .rfind(|p| p.sender == m.sender && !prev.contains(*p))
                    .cloned()
                {
                    m = p;
                }
                minimum.push(m);
            }
            let layer = finalizer.calculate_next_layer(&minimum);
            let support = finalizer.calculate_next_fringe_support_map(&js, &layer, &prev);
            let required: BTreeSet<_> = live.keys().cloned().collect();
            let mut supporters = Vec::new();
            let mut absent = Vec::new();
            for sender in live.keys() {
                let covered = support
                    .get(sender)
                    .is_some_and(|s| !s.is_empty() && s.values().all(|v| required.is_subset(v)));
                if covered {
                    supporters.push(sender.clone());
                } else {
                    let gaps: Vec<_> = support
                        .get(sender)
                        .into_iter()
                        .flat_map(|m| m.iter())
                        .map(|(candidate, seers)| {
                            format!(
                                "{}:{}",
                                candidate,
                                required
                                    .difference(seers)
                                    .cloned()
                                    .collect::<Vec<_>>()
                                    .join(",")
                            )
                        })
                        .collect();
                    absent.push(format!("{}[{}]", sender, gaps.join(";")));
                }
            }
            println!("block={} height={} supported_tip={} support={}/{} live={} supporters={} incomplete={}",
                c[0],height,tip,supporting,total,live.len(),supporters.join(","),absent.join("|"));
        }
        let message = state.create_message(
            c[0].clone(),
            height,
            c[3].clone(),
            SeqNum::try_from(c[2].parse::<i64>().unwrap()).unwrap(),
            bonds,
            &parents,
        );
        state.insert_msg_mut(&message);
    }
    let highest = state
        .msg_map
        .values()
        .flat_map(|m| m.fringe.iter())
        .map(|h| i64::from(state.msg_map[h].height))
        .max()
        .unwrap_or(0);
    println!(
        "replayed_blocks={} maximum_derived_fringe_height={}",
        state.msg_map.len(),
        highest
    );
}
