//! The stance map (GEO-3146), fitted and evaluated in shadow.
//!
//! Each voter and each claim gets a position on `k` shared axes, plus a tendency term each, and
//!
//! ```text
//! P(user u agrees with claim c) = sigmoid(mu + b_u + b_c + x_u . y_c)
//! ```
//!
//! That is low-rank logistic matrix factorisation: a one-parameter-per-axis logistic model, the
//! same shape as an ideal-point model in political science. It is chosen because it is the
//! smallest model that can answer the question the ticket asks (are two users who never voted on
//! the same claim on the same side?) while being fitted from ~5k votes. `k = 0` is the
//! "tendencies only" baseline the map has to beat: each user's habit of agreeing and each claim's
//! general popularity, with no shared axes at all.
//!
//! Fitting is alternating Newton: with the claims held fixed, each user's (bias, position) is a
//! small ridge-regularised logistic regression in `k + 1` variables, solved by one Newton step per
//! sweep, and the same for each claim with the users fixed. Every sub-problem is convex, there is
//! no learning rate to tune, and the only linear algebra needed is a 4x4 Cholesky solve, so the
//! crate gains no dependency.
//!
//! Two extra inputs, both from the graph:
//!
//! * **Stated sides.** A debater who argued For a claim is recorded as agreeing with it. That
//!   counts towards their position but must not raise their overall agree tendency (acceptance
//!   criterion), so a stated observation leaves `b_u` out of its logit and out of its gradient.
//! * **Seeds.** An extracted claim that Supports a debate's main claim is pulled towards the main
//!   claim's position, one that Opposes it towards the mirror image, through the centre of its
//!   ridge prior. With no votes on it, a seeded claim's position is its seed.
//!
//! Nothing here is stored: positions live in memory for one run and only aggregate metrics leave
//! (see migration 0108 and the `stance_map_shadow` binary).

use serde::Serialize;

pub const MODEL_VERSION: &str = "logmf-v1";

/// One Agree/Disagree observation. Indices are dense, per run.
#[derive(Debug, Clone, Copy)]
pub struct Obs {
    pub user: usize,
    pub claim: usize,
    pub agree: bool,
    /// Account weight (0101); stated sides are also scaled by `stated_weight`.
    pub weight: f64,
    /// A debater's stated side rather than a vote.
    pub stated: bool,
}

/// `claim` Supports (`sign = 1`) or Opposes (`sign = -1`) `main`.
#[derive(Debug, Clone, Copy)]
pub struct Seed {
    pub claim: usize,
    pub main: usize,
    pub sign: f64,
}

#[derive(Debug, Clone)]
pub struct Dataset {
    pub n_users: usize,
    pub n_claims: usize,
    pub obs: Vec<Obs>,
    pub seeds: Vec<Seed>,
}

#[derive(Debug, Clone, Copy)]
pub struct FitParams {
    pub k: usize,
    /// Ridge strength on positions (users and claims).
    pub lambda: f64,
    /// Ridge strength on the tendency terms.
    pub lambda_bias: f64,
    pub sweeps: usize,
    pub seed: u64,
    pub use_seeds: bool,
}

#[derive(Debug, Clone)]
pub struct Model {
    pub k: usize,
    pub mu: f64,
    pub user_bias: Vec<f64>,
    pub claim_bias: Vec<f64>,
    /// Row-major, `n_users x k`.
    pub user_pos: Vec<f64>,
    /// Row-major, `n_claims x k`.
    pub claim_pos: Vec<f64>,
}

impl Model {
    pub fn logit(&self, o: &Obs) -> f64 {
        let k = self.k;
        let ub = if o.stated {
            0.0
        } else {
            self.user_bias[o.user]
        };
        let dot: f64 = (0..k)
            .map(|a| self.user_pos[o.user * k + a] * self.claim_pos[o.claim * k + a])
            .sum();
        self.mu + ub + self.claim_bias[o.claim] + dot
    }

    pub fn claim_row(&self, c: usize) -> &[f64] {
        &self.claim_pos[c * self.k..(c + 1) * self.k]
    }
}

/// splitmix64: a small, well-mixed, deterministic generator, so runs are reproducible and the crate
/// needs no `rand`.
#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    /// Uniform in [0, 1).
    pub fn uniform(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
    pub fn normal(&mut self) -> f64 {
        let u1 = self.uniform().max(1e-300);
        let u2 = self.uniform();
        (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos()
    }
    pub fn shuffle<T>(&mut self, xs: &mut [T]) {
        for i in (1..xs.len()).rev() {
            let j = (self.next_u64() % (i as u64 + 1)) as usize;
            xs.swap(i, j);
        }
    }
}

fn sigmoid(z: f64) -> f64 {
    if z >= 0.0 {
        1.0 / (1.0 + (-z).exp())
    } else {
        let e = z.exp();
        e / (1.0 + e)
    }
}

/// Solves `h x = g` for symmetric positive-definite `h` (`d x d`, row-major) by Cholesky. `h` is
/// always PD here because every block carries a ridge term.
fn solve_spd(h: &mut [f64], g: &[f64], d: usize) -> Vec<f64> {
    for j in 0..d {
        let mut s = h[j * d + j];
        for p in 0..j {
            s -= h[j * d + p] * h[j * d + p];
        }
        let l = s.max(1e-12).sqrt();
        h[j * d + j] = l;
        for i in (j + 1)..d {
            let mut s = h[i * d + j];
            for p in 0..j {
                s -= h[i * d + p] * h[j * d + p];
            }
            h[i * d + j] = s / l;
        }
    }
    let mut y = vec![0.0; d];
    for i in 0..d {
        let mut s = g[i];
        for p in 0..i {
            s -= h[i * d + p] * y[p];
        }
        y[i] = s / h[i * d + i];
    }
    let mut x = vec![0.0; d];
    for i in (0..d).rev() {
        let mut s = y[i];
        for p in (i + 1)..d {
            s -= h[p * d + i] * x[p];
        }
        x[i] = s / h[i * d + i];
    }
    x
}

/// Fits the map. `obs` may be any subset of a dataset's observations (a training split); users and
/// claims with no observation keep their prior (zero, or the seed for a seeded claim).
pub fn fit(n_users: usize, n_claims: usize, obs: &[Obs], seeds: &[Seed], p: &FitParams) -> Model {
    let k = p.k;
    let mut rng = Rng::new(p.seed);
    let init =
        |rng: &mut Rng, n: usize| -> Vec<f64> { (0..n).map(|_| 0.1 * rng.normal()).collect() };
    let (sw, sy) = obs.iter().fold((0.0, 0.0), |(w, y), o| {
        (w + o.weight, y + if o.agree { o.weight } else { 0.0 })
    });
    let rate = ((sy + 0.5) / (sw + 1.0)).clamp(1e-3, 1.0 - 1e-3);
    let mut m = Model {
        k,
        mu: (rate / (1.0 - rate)).ln(),
        user_bias: vec![0.0; n_users],
        claim_bias: vec![0.0; n_claims],
        user_pos: init(&mut rng, n_users * k),
        claim_pos: init(&mut rng, n_claims * k),
    };

    let mut by_user: Vec<Vec<usize>> = vec![Vec::new(); n_users];
    let mut by_claim: Vec<Vec<usize>> = vec![Vec::new(); n_claims];
    for (i, o) in obs.iter().enumerate() {
        by_user[o.user].push(i);
        by_claim[o.claim].push(i);
    }
    // Each claim's seeds, kept only when the claim has no contradictory seed to the same main.
    let mut seeds_of: Vec<Vec<(usize, f64)>> = vec![Vec::new(); n_claims];
    if p.use_seeds && k > 0 {
        for s in seeds {
            if s.claim < n_claims && s.main < n_claims && s.claim != s.main {
                seeds_of[s.claim].push((s.main, s.sign));
            }
        }
    }

    let d = k + 1;
    let mut h = vec![0.0; d * d];
    let mut g = vec![0.0; d];
    let mut x = vec![0.0; d];
    for _ in 0..p.sweeps {
        // Global intercept.
        let (mut gm, mut hm) = (0.0, 1e-6);
        for o in obs {
            let pr = sigmoid(m.logit(o));
            gm += o.weight * (pr - if o.agree { 1.0 } else { 0.0 });
            hm += o.weight * pr * (1.0 - pr);
        }
        m.mu -= gm / hm;

        // Users: (b_u, x_u) with the claims fixed.
        for (u, rows) in by_user.iter().enumerate() {
            if rows.is_empty() {
                continue;
            }
            h.iter_mut().for_each(|v| *v = 0.0);
            g.iter_mut().for_each(|v| *v = 0.0);
            for &i in rows {
                let o = &obs[i];
                let pr = sigmoid(m.logit(o));
                let r = o.weight * (pr - if o.agree { 1.0 } else { 0.0 });
                let c = o.weight * pr * (1.0 - pr);
                // A stated side does not touch the user's tendency.
                x[0] = if o.stated { 0.0 } else { 1.0 };
                x[1..].copy_from_slice(m.claim_row(o.claim));
                for a in 0..d {
                    g[a] += r * x[a];
                    for b in 0..d {
                        h[a * d + b] += c * x[a] * x[b];
                    }
                }
            }
            g[0] += p.lambda_bias * m.user_bias[u];
            h[0] += p.lambda_bias;
            for a in 0..k {
                g[a + 1] += p.lambda * m.user_pos[u * k + a];
                h[(a + 1) * d + a + 1] += p.lambda;
            }
            let step = solve_spd(&mut h, &g, d);
            m.user_bias[u] -= step[0];
            for a in 0..k {
                m.user_pos[u * k + a] -= step[a + 1];
            }
        }

        // Claims: (b_c, y_c) with the users fixed; the ridge is centred on the seed.
        let mut centre = vec![0.0; k];
        for c in 0..n_claims {
            centre.iter_mut().for_each(|v| *v = 0.0);
            if !seeds_of[c].is_empty() {
                for &(main, sign) in &seeds_of[c] {
                    for (a, cv) in centre.iter_mut().enumerate() {
                        *cv += sign * m.claim_pos[main * k + a];
                    }
                }
                let n = seeds_of[c].len() as f64;
                centre.iter_mut().for_each(|v| *v /= n);
            }
            if by_claim[c].is_empty() {
                // No observation: the position is the prior's centre.
                m.claim_pos[c * k..(c + 1) * k].copy_from_slice(&centre);
                continue;
            }
            h.iter_mut().for_each(|v| *v = 0.0);
            g.iter_mut().for_each(|v| *v = 0.0);
            for &i in &by_claim[c] {
                let o = &obs[i];
                let pr = sigmoid(m.logit(o));
                let r = o.weight * (pr - if o.agree { 1.0 } else { 0.0 });
                let w = o.weight * pr * (1.0 - pr);
                x[0] = 1.0;
                x[1..].copy_from_slice(&m.user_pos[o.user * k..(o.user + 1) * k]);
                for a in 0..d {
                    g[a] += r * x[a];
                    for b in 0..d {
                        h[a * d + b] += w * x[a] * x[b];
                    }
                }
            }
            g[0] += p.lambda_bias * m.claim_bias[c];
            h[0] += p.lambda_bias;
            for a in 0..k {
                g[a + 1] += p.lambda * (m.claim_pos[c * k + a] - centre[a]);
                h[(a + 1) * d + a + 1] += p.lambda;
            }
            let step = solve_spd(&mut h, &g, d);
            m.claim_bias[c] -= step[0];
            for a in 0..k {
                m.claim_pos[c * k + a] -= step[a + 1];
            }
        }
    }
    m
}

/// ROC AUC of `scores` for `labels` (Mann-Whitney, ties counted half). None without both classes.
pub fn auc(scores: &[f64], labels: &[bool]) -> Option<f64> {
    let n = scores.len();
    let pos = labels.iter().filter(|&&l| l).count();
    let neg = n - pos;
    if pos == 0 || neg == 0 {
        return None;
    }
    let mut idx: Vec<usize> = (0..n).collect();
    idx.sort_by(|&a, &b| scores[a].total_cmp(&scores[b]));
    let mut rank_sum = 0.0;
    let mut i = 0;
    while i < n {
        let mut j = i;
        while j + 1 < n && scores[idx[j + 1]] == scores[idx[i]] {
            j += 1;
        }
        let avg = (i + j) as f64 / 2.0 + 1.0;
        for &t in &idx[i..=j] {
            if labels[t] {
                rank_sum += avg;
            }
        }
        i = j + 1;
    }
    Some((rank_sum - (pos * (pos + 1)) as f64 / 2.0) / (pos as f64 * neg as f64))
}

pub fn pearson(a: &[f64], b: &[f64]) -> Option<f64> {
    let n = a.len();
    if n < 3 || b.len() != n {
        return None;
    }
    let ma = a.iter().sum::<f64>() / n as f64;
    let mb = b.iter().sum::<f64>() / n as f64;
    let (mut sab, mut saa, mut sbb) = (0.0, 0.0, 0.0);
    for i in 0..n {
        sab += (a[i] - ma) * (b[i] - mb);
        saa += (a[i] - ma) * (a[i] - ma);
        sbb += (b[i] - mb) * (b[i] - mb);
    }
    if saa <= 1e-18 || sbb <= 1e-18 {
        return None;
    }
    Some(sab / (saa * sbb).sqrt())
}

/// Each listed claim's coordinate on the model's main axis: the first principal component of
/// their positions. Rotation-invariant, so two separately fitted maps can be compared even though
/// their axes come out in arbitrary orientations.
pub fn main_axis_projection(m: &Model, claims: &[usize]) -> Vec<f64> {
    let k = m.k;
    if k == 0 || claims.is_empty() {
        return vec![0.0; claims.len()];
    }
    let n = claims.len() as f64;
    let mut mean = vec![0.0; k];
    for &c in claims {
        for (a, mv) in mean.iter_mut().enumerate() {
            *mv += m.claim_pos[c * k + a] / n;
        }
    }
    let mut cov = vec![0.0; k * k];
    for &c in claims {
        for a in 0..k {
            for b in 0..k {
                cov[a * k + b] +=
                    (m.claim_pos[c * k + a] - mean[a]) * (m.claim_pos[c * k + b] - mean[b]);
            }
        }
    }
    // Power iteration from a fixed start; k <= a handful, so 200 steps is plenty.
    let mut v: Vec<f64> = (0..k).map(|a| 1.0 + 0.1 * a as f64).collect();
    for _ in 0..200 {
        let mut nv = vec![0.0; k];
        for a in 0..k {
            for b in 0..k {
                nv[a] += cov[a * k + b] * v[b];
            }
        }
        let norm = nv.iter().map(|x| x * x).sum::<f64>().sqrt();
        if norm <= 1e-18 {
            break;
        }
        v = nv.into_iter().map(|x| x / norm).collect();
    }
    claims
        .iter()
        .map(|&c| {
            (0..k)
                .map(|a| (m.claim_pos[c * k + a] - mean[a]) * v[a])
                .sum()
        })
        .collect()
}

#[derive(Debug, Clone)]
pub struct EvalConfig {
    pub ks: Vec<usize>,
    pub lambdas: Vec<f64>,
    pub lambda_bias: f64,
    pub holdout: f64,
    pub repeats: usize,
    pub sweeps: usize,
    pub seed: u64,
    /// A claim enters the half-split comparison only with this many voters in each half.
    pub min_split_voters: usize,
}

impl Default for EvalConfig {
    fn default() -> Self {
        Self {
            ks: vec![1, 2, 3],
            lambdas: vec![1.0, 3.0, 10.0],
            lambda_bias: 1.0,
            holdout: 0.2,
            repeats: 3,
            sweeps: 40,
            seed: 3146,
            min_split_voters: 3,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct CandidateScore {
    pub k: usize,
    pub lambda: f64,
    pub val_auc: Option<f64>,
}

/// What one run measured. Field names match the `stance_map_runs` columns; `detail` takes the rest.
#[derive(Debug, Clone, Serialize)]
pub struct Evaluation {
    pub model_version: &'static str,
    pub clean_votes: usize,
    pub weighted_votes: f64,
    pub voters: usize,
    pub claims: usize,
    pub stated_positions: usize,
    pub seeded_claims: usize,
    pub axes: Option<usize>,
    pub lambda: Option<f64>,
    pub auc_map: Option<f64>,
    pub auc_tendencies: Option<f64>,
    pub auc_lift: Option<f64>,
    pub auc_map_unseeded: Option<f64>,
    pub test_votes: usize,
    pub split_correlation: Option<f64>,
    pub split_claims: usize,
    pub detail: EvalDetail,
}

#[derive(Debug, Clone, Serialize)]
pub struct EvalDetail {
    pub candidates: Vec<CandidateScore>,
    pub auc_map_per_repeat: Vec<Option<f64>>,
    pub auc_tendencies_per_repeat: Vec<Option<f64>>,
    pub split_correlation_per_repeat: Vec<Option<f64>>,
    pub repeats: usize,
    pub holdout: f64,
}

fn mean_some(xs: &[Option<f64>]) -> Option<f64> {
    let v: Vec<f64> = xs.iter().flatten().copied().collect();
    if v.is_empty() {
        None
    } else {
        Some(v.iter().sum::<f64>() / v.len() as f64)
    }
}

/// Moves a random `share` of the vote observations in `pool` out into a held-out set, keeping every
/// held-out vote's user and claim represented in what remains (a vote on a claim nobody else saw
/// tests the prior, not the map). Stated sides always stay in.
fn holdout_split(
    obs: &[Obs],
    pool: &[usize],
    share: f64,
    n_users: usize,
    n_claims: usize,
    rng: &mut Rng,
) -> (Vec<usize>, Vec<usize>) {
    let mut user_left = vec![0usize; n_users];
    let mut claim_left = vec![0usize; n_claims];
    for &i in pool {
        user_left[obs[i].user] += 1;
        claim_left[obs[i].claim] += 1;
    }
    let mut votes: Vec<usize> = pool.iter().copied().filter(|&i| !obs[i].stated).collect();
    rng.shuffle(&mut votes);
    let target = (votes.len() as f64 * share).round() as usize;
    let mut held = vec![false; obs.len()];
    let mut n_held = 0;
    for &i in &votes {
        if n_held >= target {
            break;
        }
        let o = &obs[i];
        if user_left[o.user] > 1 && claim_left[o.claim] > 1 {
            user_left[o.user] -= 1;
            claim_left[o.claim] -= 1;
            held[i] = true;
            n_held += 1;
        }
    }
    let train = pool.iter().copied().filter(|&i| !held[i]).collect();
    let test = pool.iter().copied().filter(|&i| held[i]).collect();
    (train, test)
}

fn score(m: &Model, obs: &[Obs], idx: &[usize]) -> Option<f64> {
    let s: Vec<f64> = idx.iter().map(|&i| m.logit(&obs[i])).collect();
    let l: Vec<bool> = idx.iter().map(|&i| obs[i].agree).collect();
    auc(&s, &l)
}

fn pick(obs: &[Obs], idx: &[usize]) -> Vec<Obs> {
    idx.iter().map(|&i| obs[i]).collect()
}

/// Fits the map on the dataset and measures it: held-out AUC against tendencies only, the seeds'
/// contribution, and half-split stability of the main axis.
pub fn evaluate(data: &Dataset, cfg: &EvalConfig) -> Evaluation {
    let obs = &data.obs;
    let (nu, nc) = (data.n_users, data.n_claims);
    let votes: Vec<&Obs> = obs.iter().filter(|o| !o.stated).collect();
    let mut voter_seen = vec![false; nu];
    let mut claim_seen = vec![false; nc];
    for o in obs {
        claim_seen[o.claim] = true;
        if !o.stated {
            voter_seen[o.user] = true;
        }
    }
    let seeded_claims = {
        let mut s = vec![false; nc];
        for sd in &data.seeds {
            if sd.claim < nc && sd.main < nc && claim_seen[sd.claim] && claim_seen[sd.main] {
                s[sd.claim] = true;
            }
        }
        s.iter().filter(|&&b| b).count()
    };
    let params = |k: usize, lambda: f64, seed: u64, use_seeds: bool| FitParams {
        k,
        lambda,
        lambda_bias: cfg.lambda_bias,
        sweeps: cfg.sweeps,
        seed,
        use_seeds,
    };

    // Splits per repeat: train/test, then train -> fit/validation.
    let all: Vec<usize> = (0..obs.len()).collect();
    let mut splits = Vec::new();
    for r in 0..cfg.repeats {
        let mut rng = Rng::new(cfg.seed.wrapping_add(r as u64 * 7919));
        let (train, test) = holdout_split(obs, &all, cfg.holdout, nu, nc, &mut rng);
        let (fit_idx, val) = holdout_split(obs, &train, cfg.holdout, nu, nc, &mut rng);
        splits.push((train, test, fit_idx, val));
    }

    // Model selection on validation only.
    let mut candidates = Vec::new();
    for &k in &cfg.ks {
        for &lambda in &cfg.lambdas {
            let per: Vec<Option<f64>> = splits
                .iter()
                .enumerate()
                .map(|(r, (_, _, fit_idx, val))| {
                    let m = fit(
                        nu,
                        nc,
                        &pick(obs, fit_idx),
                        &data.seeds,
                        &params(k, lambda, r as u64, true),
                    );
                    // Indices into `obs` are still valid: `m` scores any observation.
                    score(&m, obs, val)
                })
                .collect();
            candidates.push(CandidateScore {
                k,
                lambda,
                val_auc: mean_some(&per),
            });
        }
    }
    let best = candidates
        .iter()
        .filter(|c| c.val_auc.is_some())
        .max_by(|a, b| a.val_auc.unwrap().total_cmp(&b.val_auc.unwrap()))
        .cloned();

    let mut auc_map = Vec::new();
    let mut auc_base = Vec::new();
    let mut auc_unseeded = Vec::new();
    let mut split_corr = Vec::new();
    let mut split_claims = Vec::new();
    let test_votes = splits.first().map(|s| s.1.len()).unwrap_or(0);
    if let Some(b) = &best {
        for (r, (train, test, _, _)) in splits.iter().enumerate() {
            let tr = pick(obs, train);
            let seed = 1000 + r as u64;
            let m = fit(nu, nc, &tr, &data.seeds, &params(b.k, b.lambda, seed, true));
            auc_map.push(score(&m, obs, test));
            let m0 = fit(nu, nc, &tr, &data.seeds, &params(0, b.lambda, seed, false));
            auc_base.push(score(&m0, obs, test));
            let mu = fit(
                nu,
                nc,
                &tr,
                &data.seeds,
                &params(b.k, b.lambda, seed, false),
            );
            auc_unseeded.push(score(&mu, obs, test));

            // Half-split: two disjoint halves of the users, each fitted on its own.
            let mut rng = Rng::new(cfg.seed ^ 0x5eed ^ ((r as u64) << 8));
            let mut users: Vec<usize> = (0..nu).collect();
            rng.shuffle(&mut users);
            let mut half = vec![0u8; nu];
            for (i, &u) in users.iter().enumerate() {
                half[u] = (i % 2) as u8;
            }
            let (oa, ob): (Vec<Obs>, Vec<Obs>) = obs.iter().partition(|o| half[o.user] == 0);
            let ma = fit(
                nu,
                nc,
                &oa,
                &data.seeds,
                &params(b.k, b.lambda, seed + 1, true),
            );
            let mb = fit(
                nu,
                nc,
                &ob,
                &data.seeds,
                &params(b.k, b.lambda, seed + 2, true),
            );
            let mut va = vec![0usize; nc];
            let mut vb = vec![0usize; nc];
            for o in obs.iter().filter(|o| !o.stated) {
                if half[o.user] == 0 {
                    va[o.claim] += 1;
                } else {
                    vb[o.claim] += 1;
                }
            }
            let shared: Vec<usize> = (0..nc)
                .filter(|&c| va[c] >= cfg.min_split_voters && vb[c] >= cfg.min_split_voters)
                .collect();
            split_claims.push(shared.len());
            let pa = main_axis_projection(&ma, &shared);
            let pb = main_axis_projection(&mb, &shared);
            split_corr.push(pearson(&pa, &pb).map(f64::abs));
        }
    }

    let am = mean_some(&auc_map);
    let ab = mean_some(&auc_base);
    Evaluation {
        model_version: MODEL_VERSION,
        clean_votes: votes.len(),
        weighted_votes: votes.iter().map(|o| o.weight).sum(),
        voters: voter_seen.iter().filter(|&&b| b).count(),
        claims: claim_seen.iter().filter(|&&b| b).count(),
        stated_positions: obs.iter().filter(|o| o.stated).count(),
        seeded_claims,
        axes: best.as_ref().map(|b| b.k),
        lambda: best.as_ref().map(|b| b.lambda),
        auc_map: am,
        auc_tendencies: ab,
        auc_lift: am.zip(ab).map(|(a, b)| a - b),
        auc_map_unseeded: mean_some(&auc_unseeded),
        test_votes,
        split_correlation: mean_some(&split_corr),
        split_claims: if split_claims.is_empty() {
            0
        } else {
            split_claims.iter().sum::<usize>() / split_claims.len()
        },
        detail: EvalDetail {
            candidates,
            auc_map_per_repeat: auc_map,
            auc_tendencies_per_repeat: auc_base,
            split_correlation_per_repeat: split_corr,
            repeats: cfg.repeats,
            holdout: cfg.holdout,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Users and claims on `k` planted axes, with tendencies, each user voting on a random
    /// `per_user` of the claims. `signal` scales the axes: 0 gives tendencies only.
    fn synthetic(
        nu: usize,
        nc: usize,
        k: usize,
        per_user: usize,
        signal: f64,
        seed: u64,
    ) -> (Dataset, Vec<f64>) {
        let mut rng = Rng::new(seed);
        let up: Vec<f64> = (0..nu * k).map(|_| rng.normal()).collect();
        let cp: Vec<f64> = (0..nc * k).map(|_| signal * rng.normal()).collect();
        let ub: Vec<f64> = (0..nu).map(|_| 0.5 * rng.normal()).collect();
        let cb: Vec<f64> = (0..nc).map(|_| 0.5 * rng.normal()).collect();
        let mut obs = Vec::new();
        let mut claims: Vec<usize> = (0..nc).collect();
        for u in 0..nu {
            rng.shuffle(&mut claims);
            for &c in claims.iter().take(per_user) {
                let z = 0.4
                    + ub[u]
                    + cb[c]
                    + (0..k).map(|a| up[u * k + a] * cp[c * k + a]).sum::<f64>();
                obs.push(Obs {
                    user: u,
                    claim: c,
                    agree: rng.uniform() < sigmoid(z),
                    weight: 1.0,
                    stated: false,
                });
            }
        }
        (
            Dataset {
                n_users: nu,
                n_claims: nc,
                obs,
                seeds: vec![],
            },
            cp,
        )
    }

    fn quick() -> EvalConfig {
        EvalConfig {
            ks: vec![1, 2],
            lambdas: vec![1.0, 3.0],
            repeats: 2,
            sweeps: 30,
            ..Default::default()
        }
    }

    #[test]
    fn recovers_a_planted_axis() {
        let (data, truth) = synthetic(400, 120, 1, 40, 1.5, 1);
        let m = fit(
            400,
            120,
            &data.obs,
            &[],
            &FitParams {
                k: 1,
                lambda: 1.0,
                lambda_bias: 1.0,
                sweeps: 50,
                seed: 0,
                use_seeds: false,
            },
        );
        let r = pearson(&m.claim_pos, &truth).unwrap().abs();
        assert!(r > 0.9, "planted axis recovered with |r| = {r}");
    }

    #[test]
    fn recovers_two_planted_axes_as_a_subspace() {
        let (data, truth) = synthetic(600, 100, 2, 50, 1.5, 2);
        let m = fit(
            600,
            100,
            &data.obs,
            &[],
            &FitParams {
                k: 2,
                lambda: 1.0,
                lambda_bias: 1.0,
                sweeps: 60,
                seed: 0,
                use_seeds: false,
            },
        );
        // Each true axis should be (nearly) a linear combination of the two fitted ones: regress it
        // on them and check R^2.
        for a in 0..2 {
            let y: Vec<f64> = (0..100).map(|c| truth[c * 2 + a]).collect();
            let mut xtx = [0.0; 9];
            let mut xty = [0.0; 3];
            for (c, yc) in y.iter().enumerate() {
                let x = [1.0, m.claim_pos[c * 2], m.claim_pos[c * 2 + 1]];
                for i in 0..3 {
                    xty[i] += x[i] * yc;
                    for j in 0..3 {
                        xtx[i * 3 + j] += x[i] * x[j];
                    }
                }
            }
            let beta = solve_spd(&mut xtx, &xty, 3);
            let my = y.iter().sum::<f64>() / 100.0;
            let (mut ss_res, mut ss_tot) = (0.0, 0.0);
            for (c, yc) in y.iter().enumerate() {
                let pred =
                    beta[0] + beta[1] * m.claim_pos[c * 2] + beta[2] * m.claim_pos[c * 2 + 1];
                ss_res += (yc - pred).powi(2);
                ss_tot += (yc - my).powi(2);
            }
            let r2 = 1.0 - ss_res / ss_tot;
            assert!(r2 > 0.8, "axis {a}: R^2 = {r2}");
        }
    }

    #[test]
    fn evaluation_sees_a_real_map_and_clears_stability() {
        let (data, _) = synthetic(400, 120, 1, 40, 1.5, 3);
        let e = evaluate(&data, &quick());
        let lift = e.auc_lift.unwrap();
        assert!(lift > 0.05, "a planted axis beats tendencies: lift {lift}");
        let s = e.split_correlation.unwrap();
        assert!(s > 0.8, "a planted axis is stable between halves: {s}");
        assert_eq!(e.clean_votes, 400 * 40);
        assert_eq!(e.voters, 400);
    }

    #[test]
    fn evaluation_finds_nothing_in_tendencies_only_data() {
        let (data, _) = synthetic(400, 120, 1, 40, 0.0, 4);
        let e = evaluate(&data, &quick());
        assert!(
            e.auc_lift.unwrap() < 0.02,
            "no axes planted, lift {:?}",
            e.auc_lift
        );
        assert!(
            e.split_correlation.unwrap() < 0.5,
            "no axes planted, stability {:?}",
            e.split_correlation
        );
    }

    #[test]
    fn stated_sides_move_position_but_not_tendency() {
        // Claim 0 splits the population; user 2 only ever stated a side on it.
        let mut obs = Vec::new();
        for u in 0..2 {
            for c in 0..4 {
                let agree = (c % 2 == 0) == (u == 0);
                obs.push(Obs {
                    user: u,
                    claim: c,
                    agree,
                    weight: 1.0,
                    stated: false,
                });
            }
        }
        obs.push(Obs {
            user: 2,
            claim: 0,
            agree: true,
            weight: 1.0,
            stated: true,
        });
        let m = fit(
            3,
            4,
            &obs,
            &[],
            &FitParams {
                k: 1,
                lambda: 0.5,
                lambda_bias: 1.0,
                sweeps: 50,
                seed: 0,
                use_seeds: false,
            },
        );
        assert_eq!(
            m.user_bias[2], 0.0,
            "a stated side never raises the agree tendency"
        );
        // User 2 is placed on user 0's side of claim 0.
        assert!(m.user_pos[2] * m.user_pos[0] > 0.0);
        assert!(m.user_pos[2] * m.user_pos[1] < 0.0);
    }

    #[test]
    fn seeds_place_claims_relative_to_their_main_claim() {
        let (mut data, _) = synthetic(300, 40, 1, 30, 1.5, 5);
        // Claims 40 and 41 have no votes: one Supports claim 0, one Opposes it.
        data.n_claims = 42;
        data.seeds = vec![
            Seed {
                claim: 40,
                main: 0,
                sign: 1.0,
            },
            Seed {
                claim: 41,
                main: 0,
                sign: -1.0,
            },
        ];
        let m = fit(
            300,
            42,
            &data.obs,
            &data.seeds,
            &FitParams {
                k: 1,
                lambda: 1.0,
                lambda_bias: 1.0,
                sweeps: 40,
                seed: 0,
                use_seeds: true,
            },
        );
        assert!(m.claim_pos[0].abs() > 0.1);
        assert!((m.claim_pos[40] - m.claim_pos[0]).abs() < 1e-9);
        assert!((m.claim_pos[41] + m.claim_pos[0]).abs() < 1e-9);
    }

    #[test]
    fn auc_matches_hand_computation() {
        assert_eq!(
            auc(&[0.1, 0.4, 0.35, 0.8], &[false, false, true, true]),
            Some(0.75)
        );
        assert_eq!(auc(&[1.0, 1.0], &[true, false]), Some(0.5));
        assert_eq!(auc(&[1.0], &[true]), None);
    }

    #[test]
    fn evaluation_is_deterministic() {
        let (data, _) = synthetic(120, 40, 1, 20, 1.0, 6);
        let a = evaluate(&data, &quick());
        let b = evaluate(&data, &quick());
        assert_eq!(a.auc_map, b.auc_map);
        assert_eq!(a.split_correlation, b.split_correlation);
    }
}
