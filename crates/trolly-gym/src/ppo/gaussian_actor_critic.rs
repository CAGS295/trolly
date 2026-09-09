//! Tanh-Gaussian MLP actor-critic for the WP-033 ladder inventory MDP.
//!
//! The policy outputs target inventory `a ∈ (-1, 1)` via a squashed Gaussian:
//! `z ~ N(μ, σ)`, `a = tanh(z)`. Matrix-game / live `Action` heads stay
//! categorical on [`super::ActorCritic`].

use tch::{nn, nn::Module, Kind, Tensor};

use super::config::PpoConfig;

/// Shared MLP trunk → `(μ, log σ, V)` for a 1-D tanh-Gaussian policy.
pub struct GaussianActorCritic {
    shared: Vec<nn::Linear>,
    mu_head: nn::Linear,
    log_std_head: nn::Linear,
    value_head: nn::Linear,
}

impl GaussianActorCritic {
    /// Build the network and register parameters in `vs`.
    ///
    /// `obs_dim` is the flattened ladder frame (`V × 5`).
    pub fn new(vs: &nn::VarStore, obs_dim: i64, config: &PpoConfig) -> Self {
        let p = vs.root();
        let mut shared = Vec::new();
        let mut in_dim = obs_dim;
        for (i, &h) in config.hidden_sizes.iter().enumerate() {
            shared.push(nn::linear(
                &p / format!("gauss_shared_{i}"),
                in_dim,
                h,
                Default::default(),
            ));
            in_dim = h;
        }
        Self {
            mu_head: nn::linear(&p / "gauss_mu", in_dim, 1, Default::default()),
            log_std_head: nn::linear(&p / "gauss_log_std", in_dim, 1, Default::default()),
            value_head: nn::linear(&p / "gauss_value", in_dim, 1, Default::default()),
            shared,
        }
    }

    pub fn device(&self) -> tch::Device {
        self.mu_head.ws.device()
    }

    fn trunk(&self, obs: &Tensor) -> Tensor {
        let mut x = obs.to_kind(Kind::Float);
        for layer in &self.shared {
            x = layer.forward(&x).tanh();
        }
        x
    }

    /// Forward pass: `(mu [batch], log_std [batch], values [batch])`.
    pub fn forward(&self, obs: &Tensor) -> (Tensor, Tensor, Tensor) {
        let h = self.trunk(obs);
        let mu = self.mu_head.forward(&h).squeeze_dim(-1);
        let log_std = self
            .log_std_head
            .forward(&h)
            .squeeze_dim(-1)
            .clamp(-5.0, 2.0);
        let value = self.value_head.forward(&h).squeeze_dim(-1);
        (mu, log_std, value)
    }

    /// Deterministic mean action `tanh(μ)` in `(-1, 1)`.
    pub fn mean_action(&self, obs: &Tensor) -> Tensor {
        let (mu, _, _) = self.forward(obs);
        mu.tanh()
    }

    /// Sample `a = tanh(z)`, `z ~ N(μ, σ)` and the change-of-variables log-prob.
    ///
    /// Returns `(actions [batch], log_probs [batch], values [batch])`.
    pub fn action_and_log_prob(&self, obs: &Tensor) -> (Tensor, Tensor, Tensor) {
        let (mu, log_std, value) = self.forward(obs);
        let std = log_std.exp();
        let noise = mu.randn_like();
        let pre_tanh = &mu + &std * noise;
        let action = pre_tanh.tanh();
        let log_prob = gaussian_log_prob(&pre_tanh, &mu, &log_std) - tanh_jacobian(&pre_tanh);
        (action, log_prob, value)
    }

    /// Evaluate log π(a|s) and Gaussian entropy for stored tanh actions.
    ///
    /// Returns `(action_log_probs [batch], entropy [batch])`.
    pub fn evaluate_actions(&self, obs: &Tensor, actions: &Tensor) -> (Tensor, Tensor) {
        let (mu, log_std, _) = self.forward(obs);
        let action = actions.clamp(-0.999_999, 0.999_999);
        let pre_tanh = atanh(&action);
        let log_prob = gaussian_log_prob(&pre_tanh, &mu, &log_std) - tanh_jacobian(&pre_tanh);
        // 1-D Gaussian entropy (pre-tanh); Jacobian is constant w.r.t. θ for a given a.
        let entropy = 0.5 * (1.0 + std::f64::consts::LN_2 + std::f64::consts::PI.ln()) + &log_std;
        (log_prob, entropy)
    }
}

pub(crate) fn gaussian_log_prob(sample: &Tensor, mu: &Tensor, log_std: &Tensor) -> Tensor {
    let std = log_std.exp();
    let var = std.square() + 1e-8;
    let log_2pi = (2.0 * std::f64::consts::PI).ln();
    let quad = (sample - mu).square() / (var * 2.0);
    -quad - log_std - log_2pi * 0.5
}

pub(crate) fn tanh_jacobian(pre_tanh: &Tensor) -> Tensor {
    // log(1 − tanh(z)²) = 2 * (log(2) − z − softplus(−2z))
    let soft = (pre_tanh * (-2.0)).softplus();
    let inner = pre_tanh * (-1.0) - soft + (2.0_f64).ln();
    inner * 2.0
}

pub(crate) fn atanh(x: &Tensor) -> Tensor {
    // 0.5 * ln((1+x)/(1-x))
    let ratio = (x + 1.0) / (x * (-1.0) + 1.0);
    ratio.log() * 0.5
}

#[cfg(test)]
mod tests {
    use super::*;
    use tch::Device;

    fn model() -> (nn::VarStore, GaussianActorCritic) {
        let vs = nn::VarStore::new(Device::Cpu);
        let ac = GaussianActorCritic::new(&vs, 40, &PpoConfig::default());
        (vs, ac)
    }

    #[test]
    fn gaussian_forward_shapes() {
        let (_vs, ac) = model();
        let obs = Tensor::zeros(&[8, 40], (Kind::Float, Device::Cpu));
        let (mu, log_std, value) = ac.forward(&obs);
        assert_eq!(mu.size(), vec![8]);
        assert_eq!(log_std.size(), vec![8]);
        assert_eq!(value.size(), vec![8]);
        assert!(mu.isfinite().all().int64_value(&[]) != 0);
        assert!(log_std.isfinite().all().int64_value(&[]) != 0);
    }

    #[test]
    fn gaussian_log_prob_shapes_and_finite() {
        let (_vs, ac) = model();
        let obs = Tensor::randn(&[16, 40], (Kind::Float, Device::Cpu));
        let (action, log_prob, value) = ac.action_and_log_prob(&obs);
        assert_eq!(action.size(), vec![16]);
        assert_eq!(log_prob.size(), vec![16]);
        assert_eq!(value.size(), vec![16]);
        let max_abs = action.abs().max().double_value(&[]);
        assert!(max_abs < 1.0, "tanh action must be in (-1,1), max |a|={max_abs}");
        assert!(log_prob.isfinite().all().int64_value(&[]) != 0);
        let (eval_lp, entropy) = ac.evaluate_actions(&obs, &action);
        assert_eq!(eval_lp.size(), vec![16]);
        assert_eq!(entropy.size(), vec![16]);
        assert!(eval_lp.isfinite().all().int64_value(&[]) != 0);
        assert!(entropy.isfinite().all().int64_value(&[]) != 0);
    }

    #[test]
    fn mean_action_is_tanh_mu() {
        let (_vs, ac) = model();
        let obs = Tensor::zeros(&[4, 40], (Kind::Float, Device::Cpu));
        let mean = ac.mean_action(&obs);
        assert_eq!(mean.size(), vec![4]);
        assert!(mean.abs().max().double_value(&[]) < 1.0);
    }
}
