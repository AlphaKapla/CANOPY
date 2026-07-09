# Canopy engine — BDD quantification core (Rust)

Quantifies fault trees from the git-native PSA YAML model format.

## What it does
- Loads the YAML model (fault trees, basic events, parameters, house events),
  resolves parameter references, computes point probabilities per failure model
  (probability / rate-mission / rate-repair).
- Compiles gate formulas (and/or/xor/not/atleast, cross-tree transfers,
  house-event folding) into a reduced ordered BDD with gate-cycle detection.
- Exact top-event probability (no rare-event approximation), O(|BDD|).
- Minimal cut sets via Rauzy's minimal-solutions algorithm (coherent trees),
  ranked by probability.
- Birnbaum importance per basic event.

## Engine design
- Node = 12 bytes (var, low, high) in a flat Vec arena; u32 indices.
- Hash consing (unique table) -> canonical DAGs, O(1) equivalence.
- Memoized apply (and/or/xor) and without; operand-order normalization
  for commutative ops.
- Variable order = DFS discovery order from the top gate.

## Usage
    cargo run --release -- <model-dir> <FT-ID> [--house HE-ID=true] [--mcs-limit N]
    cargo test                    # unit tests (known-answer checks)
    cargo run --release --example bench   # 30k-event synthetic scale test

## Known limitations (v0.1, deliberate)
- No garbage collection: dead intermediate nodes stay in the arena. Fine for
  batch quantification; long-lived services need mark-sweep GC.
- No dynamic variable reordering (sifting); DFS order only.
- MCS restricted to coherent trees (prime implicants for non-coherent logic
  need Coudert-Madre / meta-products).
- No complement edges (would roughly halve node count).
- CCF groups not yet expanded (compile-time expansion belongs in the
  YAML->engine compiler pass).
- Event-tree sequence quantification not yet wired (needs the linked
  fault-tree product per sequence path).
