// Synthetic scale test: S systems, each 2-of-3 trains, each train an OR of
// C component failures. Top = OR over all systems.
use canopy::bdd::Bdd;
use std::time::Instant;

fn main() {
    let (s, c) = (2000usize, 5usize); // 2000*3*5 = 30,000 basic events
    let mut bdd = Bdd::new();
    let t0 = Instant::now();
    let mut systems: Vec<u32> = Vec::with_capacity(s);
    let mut var = 0u32;
    for _ in 0..s {
        let mut trains = Vec::with_capacity(3);
        for _ in 0..3 {
            let mut tr = canopy::bdd::ZERO;
            for _ in 0..c {
                let v = bdd.variable(var);
                var += 1;
                tr = bdd.or(tr, v);
            }
            trains.push(tr);
        }
        let sys = bdd.atleast(2, &trains);
        systems.push(sys);
    }
    // Balanced pairwise OR: O(N log N) node churn instead of the O(N^2)
    // you get from linear accumulation (each or() copies its operands'
    // structure; a growing accumulator gets recopied every iteration).
    while systems.len() > 1 {
        let mut next = Vec::with_capacity(systems.len() / 2 + 1);
        for pair in systems.chunks(2) {
            next.push(if pair.len() == 2 { bdd.or(pair[0], pair[1]) } else { pair[0] });
        }
        systems = next;
    }
    let top = systems[0];
    let build = t0.elapsed();
    let p = vec![1e-3f64; var as usize];
    let t1 = Instant::now();
    let ptop = bdd.probability(top, &p);
    let quant = t1.elapsed();
    let nodes = bdd.node_count();
    println!("basic events : {var}");
    println!("BDD nodes    : {nodes}  (~{} KiB arena)", nodes * 12 / 1024);
    println!("build        : {build:?}");
    println!("P(top)       : {ptop:.6e}  ({quant:?})");
}
