//! Reduced Ordered Binary Decision Diagram engine for fault tree analysis.
//!
//! Memory layout notes (the reason this is in Rust):
//! - A node is 12 bytes: (var: u32, low: u32, high: u32), stored in one flat
//!   `Vec<Node>` arena. References between nodes are u32 indices, not
//!   pointers: half the size of a pointer on 64-bit, and the arena is
//!   contiguous so traversals are cache-friendly.
//! - Hash consing (the "unique table") guarantees structural sharing: an
//!   identical sub-function is stored exactly once, so `mk` gives canonical,
//!   maximally-shared DAGs and O(1) equality checks (index comparison).
//! - The apply cache memoizes (op, f, g) -> result, which is what turns the
//!   naive exponential Shannon expansion into the classic
//!   O(|f|·|g|) apply algorithm.

use std::collections::HashMap;

pub const ZERO: u32 = 0;
pub const ONE: u32 = 1;
const TERMINAL_VAR: u32 = u32::MAX;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
struct Node {
    var: u32,
    low: u32,
    high: u32,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Op {
    And,
    Or,
    Xor,
    Without,
}

pub struct Bdd {
    nodes: Vec<Node>,
    unique: HashMap<Node, u32>,
    apply_cache: HashMap<(Op, u32, u32), u32>,
    not_cache: HashMap<u32, u32>,
    minsol_cache: HashMap<u32, u32>,
}

impl Bdd {
    pub fn new() -> Self {
        let mut nodes = Vec::with_capacity(1 << 16);
        // Index 0 = terminal FALSE, index 1 = terminal TRUE.
        nodes.push(Node { var: TERMINAL_VAR, low: 0, high: 0 });
        nodes.push(Node { var: TERMINAL_VAR, low: 1, high: 1 });
        Bdd {
            nodes,
            unique: HashMap::new(),
            apply_cache: HashMap::new(),
            not_cache: HashMap::new(),
            minsol_cache: HashMap::new(),
        }
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    #[inline]
    fn var(&self, f: u32) -> u32 {
        self.nodes[f as usize].var
    }
    #[inline]
    fn low(&self, f: u32) -> u32 {
        self.nodes[f as usize].low
    }
    #[inline]
    fn high(&self, f: u32) -> u32 {
        self.nodes[f as usize].high
    }
    #[inline]
    fn is_terminal(f: u32) -> bool {
        f <= ONE
    }

    /// Canonical node constructor (hash consing + redundant-node elision).
    fn mk(&mut self, var: u32, low: u32, high: u32) -> u32 {
        if low == high {
            return low; // reduction rule: redundant test
        }
        let key = Node { var, low, high };
        if let Some(&idx) = self.unique.get(&key) {
            return idx; // reduction rule: structural sharing
        }
        let idx = self.nodes.len() as u32;
        self.nodes.push(key);
        self.unique.insert(key, idx);
        idx
    }

    /// The BDD variable for basic event `var` (ordering = var index).
    pub fn variable(&mut self, var: u32) -> u32 {
        self.mk(var, ZERO, ONE)
    }

    pub fn and(&mut self, f: u32, g: u32) -> u32 {
        self.apply(Op::And, f, g)
    }
    pub fn or(&mut self, f: u32, g: u32) -> u32 {
        self.apply(Op::Or, f, g)
    }
    pub fn xor(&mut self, f: u32, g: u32) -> u32 {
        self.apply(Op::Xor, f, g)
    }

    pub fn not(&mut self, f: u32) -> u32 {
        if f == ZERO {
            return ONE;
        }
        if f == ONE {
            return ZERO;
        }
        if let Some(&r) = self.not_cache.get(&f) {
            return r;
        }
        let (v, lo, hi) = (self.var(f), self.low(f), self.high(f));
        let nl = self.not(lo);
        let nh = self.not(hi);
        let r = self.mk(v, nl, nh);
        self.not_cache.insert(f, r);
        r
    }

    /// k-of-n gate, built by dynamic programming over (index, still-needed).
    pub fn atleast(&mut self, k: usize, inputs: &[u32]) -> u32 {
        fn rec(bdd: &mut Bdd, inputs: &[u32], i: usize, k: usize) -> u32 {
            if k == 0 {
                return ONE;
            }
            if inputs.len() - i < k {
                return ZERO;
            }
            let with = rec(bdd, inputs, i + 1, k - 1);
            let without = rec(bdd, inputs, i + 1, k);
            let f = inputs[i];
            let a = bdd.and(f, with);
            let nf = bdd.not(f);
            let b = bdd.and(nf, without);
            bdd.or(a, b)
        }
        rec(self, inputs, 0, k)
    }

    /// Shannon-expansion apply with memoization.
    fn apply(&mut self, op: Op, f: u32, g: u32) -> u32 {
        // Terminal short-circuits.
        match op {
            Op::And => {
                if f == ZERO || g == ZERO {
                    return ZERO;
                }
                if f == ONE {
                    return g;
                }
                if g == ONE {
                    return f;
                }
                if f == g {
                    return f;
                }
            }
            Op::Or => {
                if f == ONE || g == ONE {
                    return ONE;
                }
                if f == ZERO {
                    return g;
                }
                if g == ZERO {
                    return f;
                }
                if f == g {
                    return f;
                }
            }
            Op::Xor => {
                if f == ZERO {
                    return g;
                }
                if g == ZERO {
                    return f;
                }
                if f == g {
                    return ZERO;
                }
                if f == ONE {
                    return self.not(g);
                }
                if g == ONE {
                    return self.not(f);
                }
            }
            Op::Without => unreachable!("without() has its own driver"),
        }
        // Commutative ops: normalize operand order for better cache hits.
        let (f, g) = if f <= g { (f, g) } else { (g, f) };
        if let Some(&r) = self.apply_cache.get(&(op, f, g)) {
            return r;
        }
        let (vf, vg) = (self.var(f), self.var(g));
        let v = vf.min(vg);
        let (f0, f1) = if vf == v {
            (self.low(f), self.high(f))
        } else {
            (f, f)
        };
        let (g0, g1) = if vg == v {
            (self.low(g), self.high(g))
        } else {
            (g, g)
        };
        let lo = self.apply(op, f0, g0);
        let hi = self.apply(op, f1, g1);
        let r = self.mk(v, lo, hi);
        self.apply_cache.insert((op, f, g), r);
        r
    }

    // ---------------------------------------------------------------------
    // Quantification
    // ---------------------------------------------------------------------

    /// Exact top-event probability. `p[i]` = probability of basic event i.
    /// P(f) = p(v)·P(high) + (1-p(v))·P(low), memoized over shared nodes,
    /// so it runs in O(|f|) — this exactness (no rare-event or min-cut-upper
    /// -bound approximation) is the whole reason BDDs are used in PSA.
    pub fn probability(&self, f: u32, p: &[f64]) -> f64 {
        let mut memo: HashMap<u32, f64> = HashMap::new();
        self.prob_rec(f, p, &mut memo)
    }

    fn prob_rec(&self, f: u32, p: &[f64], memo: &mut HashMap<u32, f64>) -> f64 {
        if f == ZERO {
            return 0.0;
        }
        if f == ONE {
            return 1.0;
        }
        if let Some(&v) = memo.get(&f) {
            return v;
        }
        let pv = p[self.var(f) as usize];
        let hi = self.prob_rec(self.high(f), p, memo);
        let lo = self.prob_rec(self.low(f), p, memo);
        let r = pv * hi + (1.0 - pv) * lo;
        memo.insert(f, r);
        r
    }

    /// Birnbaum importance of variable v: P(f | v=1) - P(f | v=0).
    pub fn birnbaum(&mut self, f: u32, v: u32, p: &[f64]) -> f64 {
        let f1 = self.restrict(f, v, true);
        let f0 = self.restrict(f, v, false);
        self.probability(f1, p) - self.probability(f0, p)
    }

    /// Cofactor f|_{v=val}.
    pub fn restrict(&mut self, f: u32, v: u32, val: bool) -> u32 {
        if Self::is_terminal(f) || self.var(f) > v {
            return f;
        }
        if self.var(f) == v {
            return if val { self.high(f) } else { self.low(f) };
        }
        let (fv, lo, hi) = (self.var(f), self.low(f), self.high(f));
        let l = self.restrict(lo, v, val);
        let h = self.restrict(hi, v, val);
        self.mk(fv, l, h)
    }

    // ---------------------------------------------------------------------
    // Minimal cut sets (Rauzy's minimal-solutions algorithm).
    // Valid for COHERENT functions (monotone: AND/OR/VOTE of positive
    // literals). The caller is responsible for checking coherence.
    // ---------------------------------------------------------------------

    /// BDD encoding exactly the minimal solutions of a monotone f.
    pub fn minsol(&mut self, f: u32) -> u32 {
        if Self::is_terminal(f) {
            return f;
        }
        if let Some(&r) = self.minsol_cache.get(&f) {
            return r;
        }
        let (v, lo, hi) = (self.var(f), self.low(f), self.high(f));
        let l = self.minsol(lo);
        let h = self.minsol(hi);
        // Solutions through the high branch are minimal only if not already
        // implied by a solution that doesn't need v at all.
        let h2 = self.without(h, l);
        let r = self.mk(v, l, h2);
        self.minsol_cache.insert(f, r);
        r
    }

    /// f ⊘ g: solutions of f that are not supersets of any solution of g.
    fn without(&mut self, f: u32, g: u32) -> u32 {
        if f == ZERO || g == ONE {
            return ZERO;
        }
        if g == ZERO || f == ONE {
            return f;
        }
        let key = (Op::Without, f, g); // NOT commutative: no normalization
        if let Some(&r) = self.apply_cache.get(&key) {
            return r;
        }
        let (vf, vg) = (self.var(f), self.var(g));
        let r = if vf < vg {
            let (lo, hi) = (self.low(f), self.high(f));
            let l = self.without(lo, g);
            let h = self.without(hi, g);
            self.mk(vf, l, h)
        } else if vf > vg {
            // g's solutions mentioning vg can't subsume sets without vg.
            let g0 = self.low(g);
            self.without(f, g0)
        } else {
            let (f0, f1) = (self.low(f), self.high(f));
            let (g0, g1) = (self.low(g), self.high(g));
            let l = self.without(f0, g0);
            // A set containing v is subsumed by g-solutions with v (g1)
            // or without v (g0): remove both.
            let t = self.without(f1, g1);
            let h = self.without(t, g0);
            self.mk(vf, l, h)
        };
        self.apply_cache.insert(key, r);
        r
    }

    /// Enumerate cut sets from a minsol BDD as sorted variable lists.
    /// `limit` caps enumeration for very large models (None = all).
    pub fn enumerate_paths(&self, f: u32, limit: Option<usize>) -> Vec<Vec<u32>> {
        let mut out = Vec::new();
        let mut path = Vec::new();
        self.paths_rec(f, &mut path, &mut out, limit);
        out
    }

    fn paths_rec(
        &self,
        f: u32,
        path: &mut Vec<u32>,
        out: &mut Vec<Vec<u32>>,
        limit: Option<usize>,
    ) {
        if let Some(l) = limit {
            if out.len() >= l {
                return;
            }
        }
        if f == ZERO {
            return;
        }
        if f == ONE {
            out.push(path.clone());
            return;
        }
        // High edge: variable is in the cut set.
        path.push(self.var(f));
        self.paths_rec(self.high(f), path, out, limit);
        path.pop();
        // Low edge: variable absent.
        self.paths_rec(self.low(f), path, out, limit);
    }
}

impl Default for Bdd {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2-train system: TOP = (A_fts OR A_ftr) AND (B_fts OR B_ftr)
    fn two_train(bdd: &mut Bdd) -> u32 {
        let a1 = bdd.variable(0);
        let a2 = bdd.variable(1);
        let b1 = bdd.variable(2);
        let b2 = bdd.variable(3);
        let ta = bdd.or(a1, a2);
        let tb = bdd.or(b1, b2);
        bdd.and(ta, tb)
    }

    #[test]
    fn probability_exact() {
        let mut bdd = Bdd::new();
        let top = two_train(&mut bdd);
        let p = vec![1e-3, 2e-3, 1e-3, 2e-3];
        // P(train) = 1-(1-1e-3)(1-2e-3); P(top) = P(train)^2 (independent)
        let ptrain = 1.0 - (1.0 - 1e-3) * (1.0 - 2e-3);
        let expect = ptrain * ptrain;
        let got = bdd.probability(top, &p);
        assert!((got - expect).abs() < 1e-15, "got {got}, expect {expect}");
    }

    #[test]
    fn mcs_two_train() {
        let mut bdd = Bdd::new();
        let top = two_train(&mut bdd);
        let ms = bdd.minsol(top);
        let mut cuts = bdd.enumerate_paths(ms, None);
        cuts.sort();
        // Exactly the 4 double cut sets {Ai, Bj}.
        assert_eq!(
            cuts,
            vec![vec![0, 2], vec![0, 3], vec![1, 2], vec![1, 3]]
        );
    }

    #[test]
    fn mcs_subsumption() {
        // TOP = A OR (A AND B): MCS must be just {A}.
        let mut bdd = Bdd::new();
        let a = bdd.variable(0);
        let b = bdd.variable(1);
        let ab = bdd.and(a, b);
        let top = bdd.or(a, ab);
        let ms = bdd.minsol(top);
        let cuts = bdd.enumerate_paths(ms, None);
        assert_eq!(cuts, vec![vec![0]]);
    }

    #[test]
    fn vote_gate() {
        // 2-of-3 with p=0.1 each: P = 3p^2(1-p) + p^3 = 0.028
        let mut bdd = Bdd::new();
        let vars: Vec<u32> = (0..3).map(|i| bdd.variable(i)).collect();
        let top = bdd.atleast(2, &vars);
        let p = vec![0.1; 3];
        let got = bdd.probability(top, &p);
        assert!((got - 0.028).abs() < 1e-12, "got {got}");
        let ms = bdd.minsol(top);
        let mut cuts = bdd.enumerate_paths(ms, None);
        cuts.sort();
        assert_eq!(cuts, vec![vec![0, 1], vec![0, 2], vec![1, 2]]);
    }

    #[test]
    fn negation_probability() {
        // Non-coherent probability still exact: P(A AND NOT B).
        let mut bdd = Bdd::new();
        let a = bdd.variable(0);
        let b = bdd.variable(1);
        let nb = bdd.not(b);
        let f = bdd.and(a, nb);
        let p = vec![0.3, 0.4];
        let got = bdd.probability(f, &p);
        assert!((got - 0.3 * 0.6).abs() < 1e-15);
    }

    #[test]
    fn hash_consing_shares_structure() {
        let mut bdd = Bdd::new();
        let a = bdd.variable(0);
        let b = bdd.variable(1);
        let f1 = bdd.and(a, b);
        let f2 = bdd.and(a, b);
        assert_eq!(f1, f2); // identical index = perfect sharing
    }
}
