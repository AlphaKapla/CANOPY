//! canopy: quantify fault trees and event trees from a git-native PSA
//! YAML model.
//!
//! Usage:
//!   canopy <model-dir> <FT-ID|ET-ID> [--house HE-ID=true ...]
//!           [--mcs-limit N] [--json]

mod bdd;
mod model;

use anyhow::{anyhow, bail, Result};
use bdd::Bdd;
use model::{Formula, FormulaOp, Model, Outcome};
use serde_json::json;
use std::collections::HashMap;
use std::path::PathBuf;

struct Compiler<'m> {
    model: &'m Model,
    bdd: Bdd,
    var_of_be: HashMap<String, u32>,
    be_of_var: Vec<String>,
    gate_cache: HashMap<String, u32>,
    in_progress: Vec<String>,
    coherent: bool,
}

impl<'m> Compiler<'m> {
    fn new(model: &'m Model) -> Self {
        Compiler {
            model,
            bdd: Bdd::new(),
            var_of_be: HashMap::new(),
            be_of_var: Vec::new(),
            gate_cache: HashMap::new(),
            in_progress: Vec::new(),
            coherent: true,
        }
    }

    fn be_var(&mut self, id: &str) -> u32 {
        if let Some(&v) = self.var_of_be.get(id) {
            return v;
        }
        let v = self.be_of_var.len() as u32;
        self.var_of_be.insert(id.to_string(), v);
        self.be_of_var.push(id.to_string());
        v
    }

    fn compile_ref(&mut self, id: &str) -> Result<u32> {
        if id.starts_with("BE-") {
            if !self.model.be_prob.contains_key(id) {
                bail!("dangling basic event reference: {id}");
            }
            let v = self.be_var(id);
            return Ok(self.bdd.variable(v));
        }
        if id.starts_with("HE-") {
            let val = self
                .model
                .house
                .get(id)
                .ok_or_else(|| anyhow!("dangling house event reference: {id}"))?;
            return Ok(if *val { bdd::ONE } else { bdd::ZERO });
        }
        if id.starts_with("GT-") {
            if let Some(&g) = self.gate_cache.get(id) {
                return Ok(g);
            }
            if self.in_progress.iter().any(|g| g == id) {
                bail!(
                    "cycle through gates: {} -> {id}",
                    self.in_progress.join(" -> ")
                );
            }
            let formula = self
                .model
                .gates
                .get(id)
                .ok_or_else(|| anyhow!("dangling gate reference: {id}"))?
                .clone();
            self.in_progress.push(id.to_string());
            let f = self.compile(&formula)?;
            self.in_progress.pop();
            self.gate_cache.insert(id.to_string(), f);
            return Ok(f);
        }
        bail!("reference with unknown prefix: {id}")
    }

    fn compile(&mut self, formula: &Formula) -> Result<u32> {
        Ok(match formula {
            Formula::Ref(id) => self.compile_ref(id)?,
            Formula::Op(op) => match op {
                FormulaOp::And(xs) => {
                    let mut acc = bdd::ONE;
                    for x in xs {
                        let f = self.compile(x)?;
                        acc = self.bdd.and(acc, f);
                    }
                    acc
                }
                FormulaOp::Or(xs) => {
                    let mut acc = bdd::ZERO;
                    for x in xs {
                        let f = self.compile(x)?;
                        acc = self.bdd.or(acc, f);
                    }
                    acc
                }
                FormulaOp::Xor(xs) => {
                    self.coherent = false;
                    let mut acc = bdd::ZERO;
                    for x in xs {
                        let f = self.compile(x)?;
                        acc = self.bdd.xor(acc, f);
                    }
                    acc
                }
                FormulaOp::Not(x) => {
                    self.coherent = false;
                    let f = self.compile(x)?;
                    self.bdd.not(f)
                }
                FormulaOp::Atleast { k, of } => {
                    let inputs: Result<Vec<u32>> =
                        of.iter().map(|x| self.compile(x)).collect();
                    self.bdd.atleast(*k, &inputs?)
                }
            },
        })
    }
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let usage = "usage: canopy <model-dir> <FT-ID|ET-ID> \
                 [--house HE-ID=bool] [--mcs-limit N] [--json]";
    let model_dir = PathBuf::from(args.next().ok_or_else(|| anyhow!(usage))?);
    let target = args.next().ok_or_else(|| anyhow!(usage))?;

    let mut mcs_limit: Option<usize> = Some(1000);
    let mut json_out = false;
    let mut prob_only = false;
    let mut house_overrides: Vec<(String, bool)> = Vec::new();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--house" => {
                let kv = args.next().ok_or_else(|| anyhow!("--house HE-ID=bool"))?;
                let (k, v) = kv
                    .split_once('=')
                    .ok_or_else(|| anyhow!("--house HE-ID=bool"))?;
                house_overrides.push((k.to_string(), v.parse()?));
            }
            "--mcs-limit" => {
                mcs_limit = Some(args.next().unwrap_or_default().parse()?);
            }
            "--json" => json_out = true,
            "--prob-only" => prob_only = true,
            other => bail!("unknown argument {other}"),
        }
    }

    let mut model = Model::load(&model_dir)?;
    for (k, v) in house_overrides {
        model.set_house(&k, v)?;
    }

    if prob_only {
        mcs_limit = Some(0);
    }
    if target.starts_with("ET-") {
        quantify_event_tree(&model_dir, model, &target, mcs_limit, json_out)
    } else {
        quantify_fault_tree(model, &target, mcs_limit, json_out, prob_only)
    }
}

fn quantify_fault_tree(
    model: Model,
    ft_id: &str,
    mcs_limit: Option<usize>,
    json_out: bool,
    prob_only: bool,
) -> Result<()> {
    let ft = model
        .fault_trees
        .get(ft_id)
        .ok_or_else(|| anyhow!("fault tree {ft_id} not found"))?;
    let top_gate = ft.top_gate.clone();

    let mut c = Compiler::new(&model);
    let top = c.compile_ref(&top_gate)?;
    let p: Vec<f64> = c.be_of_var.iter().map(|id| model.be_prob[id]).collect();
    let ptop = c.bdd.probability(top, &p);

    let mut cuts_out: Vec<(f64, Vec<String>)> = Vec::new();
    if c.coherent && mcs_limit != Some(0) {
        let ms = c.bdd.minsol(top);
        let cuts = c.bdd.enumerate_paths(ms, mcs_limit);
        for cut in cuts {
            let cp: f64 = cut.iter().map(|&v| p[v as usize]).product();
            let names = cut
                .iter()
                .map(|&v| c.be_of_var[v as usize].clone())
                .collect();
            cuts_out.push((cp, names));
        }
        cuts_out.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
    }

    let mut imp: Vec<(String, f64)> = if prob_only {
        Vec::new()
    } else {
        (0..c.be_of_var.len() as u32)
            .map(|v| (c.be_of_var[v as usize].clone(),
                      c.bdd.birnbaum(top, v, &p)))
            .collect()
    };
    imp.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());

    if json_out {
        let out = json!({
            "type": "fault_tree",
            "id": ft_id,
            "top_gate": top_gate,
            "coherent": c.coherent,
            "probability": ptop,
            "bdd_nodes": c.bdd.node_count(),
            "minimal_cut_sets": cuts_out.iter().map(|(cp, names)| json!({
                "probability": cp, "events": names })).collect::<Vec<_>>(),
            "birnbaum": imp.iter().map(|(id, b)| json!({
                "event": id, "importance": b })).collect::<Vec<_>>(),
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }

    println!("fault tree      : {ft_id} (top gate {top_gate})");
    println!("basic events    : {}", c.be_of_var.len());
    println!("BDD nodes       : {}", c.bdd.node_count());
    println!("P(top) exact    : {ptop:.6e}");
    if c.coherent {
        println!("minimal cut sets: {}", cuts_out.len());
        for (cp, names) in &cuts_out {
            println!("  {:>12.4e}  {{{}}}", cp, names.join(", "));
        }
    } else {
        println!("minimal cut sets: skipped (non-coherent tree)");
    }
    println!("Birnbaum importance:");
    for (id, b) in imp {
        println!("  {:>12.4e}  {id}", b);
    }
    Ok(())
}

fn quantify_event_tree(
    model_dir: &std::path::Path,
    mut model: Model,
    et_id: &str,
    mcs_limit: Option<usize>,
    json_out: bool,
) -> Result<()> {
    let (trees, metrics) = Model::load_event_trees(model_dir)?;
    let et = trees
        .get(et_id)
        .ok_or_else(|| anyhow!("event tree {et_id} not found"))?;
    let ie_freq = et.initiating_event.frequency.value;

    let mut fe_ids: Vec<&String> = et.functional_events.keys().collect();
    fe_ids.sort();
    let mut seq_ids: Vec<&String> = et.sequences.keys().collect();
    seq_ids.sort();

    struct SeqResult {
        id: String,
        freq: f64,
        end_state: String,
        transfer: Option<String>,
        cut_sets: Vec<(f64, Vec<String>)>,
    }
    let mut results: Vec<SeqResult> = Vec::new();

    for seq_id in &seq_ids {
        let seq = &et.sequences[*seq_id];
        let saved: Vec<(String, bool)> = seq
            .house_events
            .keys()
            .map(|k| (k.clone(), model.house[k]))
            .collect();
        for (k, v) in &seq.house_events {
            model.set_house(k, *v)?;
        }

        let mut c = Compiler::new(&model);
        let mut conj = bdd::ONE;
        let mut fail_only = bdd::ONE;
        for fe in &fe_ids {
            let top_gate = et.functional_events[*fe].top_gate.clone();
            match seq.path[*fe] {
                Outcome::Bypassed => {}
                Outcome::Failure => {
                    let f = c.compile_ref(&top_gate)?;
                    conj = c.bdd.and(conj, f);
                    fail_only = c.bdd.and(fail_only, f);
                }
                Outcome::Success => {
                    let f = c.compile_ref(&top_gate)?;
                    let nf = c.bdd.not(f);
                    conj = c.bdd.and(conj, nf);
                }
            }
        }
        let p: Vec<f64> = c.be_of_var.iter().map(|id| model.be_prob[id]).collect();
        let p_seq = c.bdd.probability(conj, &p);
        let freq = ie_freq * p_seq;

        let mut cut_sets: Vec<(f64, Vec<String>)> = Vec::new();
        // Cut sets only for coherent sequence logic: minsol is invalid in
        // the presence of NOT/XOR (prime implicants would be required).
        // A tautological failure logic (e.g. a house event pinning a
        // functional event failed) yields the EMPTY cut set, consistent
        // with the fault-tree path: the sequence needs no component
        // failures. Suppressing it would leave a dominant sequence
        // unexplained.
        if seq.end_state != "OK" && c.coherent && mcs_limit != Some(0) {
            let ms = c.bdd.minsol(fail_only);
            for cut in c.bdd.enumerate_paths(ms, mcs_limit) {
                let cp: f64 = cut.iter().map(|&v| p[v as usize]).product();
                let names = cut
                    .iter()
                    .map(|&v| c.be_of_var[v as usize].clone())
                    .collect();
                cut_sets.push((ie_freq * cp, names));
            }
            cut_sets.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
        }

        results.push(SeqResult {
            id: (*seq_id).clone(),
            freq,
            end_state: seq.end_state.clone(),
            transfer: seq.transfer.clone(),
            cut_sets,
        });
        for (k, v) in saved {
            model.set_house(&k, v)?;
        }
    }

    let metric_totals: Vec<(String, String, f64)> = metrics
        .iter()
        .map(|m| {
            let total: f64 = results
                .iter()
                .filter(|r| m.end_states.contains(&r.end_state))
                .map(|r| r.freq)
                .sum();
            (m.id.clone(), m.label.clone(), total)
        })
        .collect();

    if json_out {
        let out = json!({
            "type": "event_tree",
            "id": et_id,
            "initiating_event": {
                "id": et.initiating_event.id,
                "frequency_per_year": ie_freq,
            },
            "sequences": results.iter().map(|r| json!({
                "id": r.id,
                "frequency_per_year": r.freq,
                "end_state": r.end_state,
                "transfer": r.transfer,
                "cut_sets": r.cut_sets.iter().map(|(f, names)| json!({
                    "frequency_per_year": f, "events": names,
                })).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
            "metrics": metric_totals.iter().map(|(id, label, v)| json!({
                "id": id, "label": label, "value_per_year": v,
            })).collect::<Vec<_>>(),
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }

    println!(
        "event tree      : {et_id}  (IE {} @ {:.3e} /yr)",
        et.initiating_event.id, ie_freq
    );
    for r in &results {
        if !r.cut_sets.is_empty() {
            println!("  {} dominant cut sets (failure logic):", r.id);
            for (f, names) in r.cut_sets.iter().take(5) {
                println!("      {:>10.3e} /yr  {{{}}}", f, names.join(", "));
            }
        }
    }
    println!("sequences:");
    for r in &results {
        let note = if r.transfer.is_some() { "  [transfer]" } else { "" };
        println!(
            "  {:<14} {:>12.4e} /yr  -> {}{note}",
            r.id, r.freq, r.end_state
        );
    }
    for (id, label, v) in &metric_totals {
        println!("{id} ({label}) : {v:.4e} /yr");
    }
    let n_xfer = results.iter().filter(|r| r.transfer.is_some()).count();
    if n_xfer > 0 {
        println!(
            "note: {n_xfer} sequence(s) transfer to other event trees and \
             are not included in the metrics above"
        );
    }
    Ok(())
}
