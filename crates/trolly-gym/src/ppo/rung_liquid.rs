//! Liquid-on-rungs Gaussian FA for the WP-034 ladder MDP.
//!
//! Consumes the WP-032 frame as `[batch, V, F]` with
//! `x_k = [v_k, α_ask, α_bid, Δα, q]`. The liquid cell is driven by **each
//! rung** (`liquid_steps = V`); `h` is reset every forward (not across env
//! steps). Matrix-game [`super::LiquidActorCritic`] stays categorical on a
//! flat vector.

use tch::{nn, nn::Module, Kind, Tensor};

use crate::observation::LADDER_FEATURES_PER_RUNG;

use super::config::PpoConfig;
use super::gaussian_actor_critic::{atanh, gaussian_log_prob, tanh_jacobian};

/// Tanh-Gaussian readout after a rung-unrolled liquid cell.
pub struct RungLiquidGaussian {
    input_drive: nn::Linear,
    recurrent_drive: nn::Linear,
    input_gate: nn::Linear,
    tau_log: Tensor,
    readout: Vec<nn::Linear>,
    mu_head: nn::Linear,
    log_std_head: nn::Linear,
    value_head: nn::Linear,
    hidden_size: i64,
    rung_count: i64,
    features_per_rung: i64,
}

impl RungLiquidGaussian {
    /// `obs_dim` must equal `rung_count * LADDER_FEATURES_PER_RUNG`.
    pub fn new(vs: &nn::VarStore, obs_dim: i64, rung_count: i64, config: &PpoConfig) -> Self {
        let features_per_rung = LADDER_FEATURES_PER_RUNG as i64;
        let rung_count = rung_count.max(1);
        assert_eq!(
            obs_dim,
            rung_count * features_per_rung,
            "rung liquid obs_dim must be V×{features_per_rung}"
        );
        let p = vs.root();
        let hidden_size = config
            .hidden_sizes
            .first()
            .copied()
            .unwrap_or_else(|| obs_dim.max(1));

        let input_drive = nn::linear(
            &p / "rung_liquid_input_drive",
            features_per_rung,
            hidden_size,
            Default::default(),
        );
        let recurrent_drive = nn::linear(
            &p / "rung_liquid_recurrent_drive",
            hidden_size,
            hidden_size,
            Default::default(),
        );
        let input_gate = nn::linear(
            &p / "rung_liquid_input_gate",
            features_per_rung,
            hidden_size,
            Default::default(),
        );
        let tau_log = p.var(
            "rung_liquid_tau_log",
            &[hidden_size],
            nn::Init::Const(0.0),
        );

        let mut readout = Vec::new();
        let mut in_dim = hidden_size;
        for (i, &h) in config.hidden_sizes.iter().skip(1).enumerate() {
            readout.push(nn::linear(
                &p / format!("rung_liquid_readout_{i}"),
                in_dim,
                h,
                Default::default(),
            ));
            in_dim = h;
        }

        Self {
            input_drive,
            recurrent_drive,
            input_gate,
            tau_log,
            readout,
            mu_head: nn::linear(&p / "rung_liquid_mu", in_dim, 1, Default::default()),
            log_std_head: nn::linear(&p / "rung_liquid_log_std", in_dim, 1, Default::default()),
            value_head: nn::linear(&p / "rung_liquid_value", in_dim, 1, Default::default()),
            hidden_size,
            rung_count,
            features_per_rung,
        }
    }

    pub fn device(&self) -> tch::Device {
        self.mu_head.ws.device()
    }

    fn rungs(&self, obs: &Tensor) -> Tensor {
        let x = obs.to_kind(Kind::Float);
        let batch = x.size()[0];
        x.view([batch, self.rung_count, self.features_per_rung])
    }

    /// Rung width `Δv` from adjacent `v` coordinates (feature 0). Defaults to 1.
    fn rung_width(rungs: &Tensor) -> Tensor {
        let v_len = rungs.size()[1];
        if v_len < 2 {
            return Tensor::from(1.0).to_device(rungs.device()).to_kind(Kind::Float);
        }
        let v0 = rungs.narrow(1, 0, 1).narrow(2, 0, 1);
        let v1 = rungs.narrow(1, 1, 1).narrow(2, 0, 1);
        (v1 - v0).abs().mean(Kind::Float).clamp(1e-6, 10.0)
    }

    fn encode(&self, obs: &Tensor) -> Tensor {
        let rungs = self.rungs(obs);
        let batch = rungs.size()[0];
        let mut state = Tensor::zeros(&[batch, self.hidden_size], (Kind::Float, rungs.device()));
        let tau = self.tau_log.exp() + 1.0;
        let dv = Self::rung_width(&rungs);

        for k in 0..self.rung_count {
            let xk = rungs
                .narrow(1, k, 1)
                .squeeze_dim(1)
                .to_kind(Kind::Float);
            let input_gate = self.input_gate.forward(&xk).sigmoid();
            // Optional Δv-scaled gate so the unroll tracks ∫ α(v) dv.
            let alpha = ((&input_gate / &tau) * &dv).clamp(0.0, 1.0);
            let candidate =
                (self.input_drive.forward(&xk) + self.recurrent_drive.forward(&state)).tanh();
            state = &state + &alpha * (candidate - &state);
        }

        let mut features = state;
        for layer in &self.readout {
            features = layer.forward(&features).tanh();
        }
        features
    }

    /// `(mu [batch], log_std [batch], values [batch])`.
    pub fn forward(&self, obs: &Tensor) -> (Tensor, Tensor, Tensor) {
        let h = self.encode(obs);
        let mu = self.mu_head.forward(&h).squeeze_dim(-1);
        let log_std = self
            .log_std_head
            .forward(&h)
            .squeeze_dim(-1)
            .clamp(-5.0, 2.0);
        let value = self.value_head.forward(&h).squeeze_dim(-1);
        (mu, log_std, value)
    }

    pub fn mean_action(&self, obs: &Tensor) -> Tensor {
        let (mu, _, _) = self.forward(obs);
        mu.tanh()
    }

    pub fn action_and_log_prob(&self, obs: &Tensor) -> (Tensor, Tensor, Tensor) {
        let (mu, log_std, value) = self.forward(obs);
        let std = log_std.exp();
        let noise = mu.randn_like();
        let pre_tanh = &mu + &std * noise;
        let action = pre_tanh.tanh();
        let log_prob = gaussian_log_prob(&pre_tanh, &mu, &log_std) - tanh_jacobian(&pre_tanh);
        (action, log_prob, value)
    }

    pub fn evaluate_actions(&self, obs: &Tensor, actions: &Tensor) -> (Tensor, Tensor) {
        let (mu, log_std, _) = self.forward(obs);
        let action = actions.clamp(-0.999_999, 0.999_999);
        let pre_tanh = atanh(&action);
        let log_prob = gaussian_log_prob(&pre_tanh, &mu, &log_std) - tanh_jacobian(&pre_tanh);
        let entropy = 0.5 * (1.0 + std::f64::consts::LN_2 + std::f64::consts::PI.ln()) + &log_std;
        (log_prob, entropy)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tch::Device;

    fn model(rungs: i64) -> (nn::VarStore, RungLiquidGaussian) {
        let vs = nn::VarStore::new(Device::Cpu);
        let obs_dim = rungs * LADDER_FEATURES_PER_RUNG as i64;
        let ac = RungLiquidGaussian::new(&vs, obs_dim, rungs, &PpoConfig::default());
        (vs, ac)
    }

    #[test]
    fn rung_liquid_does_not_repeat_flat_x() {
        let (_vs, ac) = model(4);
        let mut obs = vec![0.0_f32; 20];
        // Distinct v and Δα on each rung so a flattened-repeat FA would mismatch.
        for k in 0..4 {
            obs[k * 5] = k as f32 * 0.25;
            obs[k * 5 + 3] = 0.0625;
        }
        let t = Tensor::from_slice(&obs).view([1, 20]);
        let (mu, log_std, value) = ac.forward(&t);
        assert_eq!(mu.size(), vec![1]);
        assert_eq!(log_std.size(), vec![1]);
        assert_eq!(value.size(), vec![1]);
        assert!(mu.isfinite().all().int64_value(&[]) != 0);
    }

    #[test]
    fn rung_liquid_log_prob_finite() {
        let (_vs, ac) = model(4);
        let obs = Tensor::randn(&[8, 20], (Kind::Float, Device::Cpu));
        let (action, log_prob, _) = ac.action_and_log_prob(&obs);
        assert!(action.abs().max().double_value(&[]) < 1.0);
        assert!(log_prob.isfinite().all().int64_value(&[]) != 0);
        let (eval_lp, entropy) = ac.evaluate_actions(&obs, &action);
        assert!(eval_lp.isfinite().all().int64_value(&[]) != 0);
        assert!(entropy.isfinite().all().int64_value(&[]) != 0);
    }
}
