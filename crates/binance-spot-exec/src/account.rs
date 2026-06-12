use std::collections::HashMap;

use crate::events::{
    AssetBalance, BalanceUpdate, ExecutionReport, ExternalLockUpdate, OutboundAccountPosition,
    SpotUserEvent,
};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct AccountBook {
    pub balances: HashMap<String, AssetBalance>,
    pub open_orders: HashMap<i64, ExecutionReport>,
}

impl AccountBook {
    pub fn apply(&mut self, event: &SpotUserEvent) {
        match event {
            SpotUserEvent::OutboundAccountPosition(pos) => self.apply_position(pos),
            SpotUserEvent::BalanceUpdate(update) => self.apply_balance_delta(update),
            SpotUserEvent::ExecutionReport(report) => self.apply_execution_report(report),
            SpotUserEvent::ExternalLockUpdate(lock) => self.apply_external_lock(lock),
            SpotUserEvent::ListStatus(_) | SpotUserEvent::EventStreamTerminated(_) => {}
        }
    }

    fn apply_position(&mut self, pos: &OutboundAccountPosition) {
        for balance in &pos.balances {
            self.balances.insert(balance.asset.clone(), balance.clone());
        }
    }

    fn apply_balance_delta(&mut self, update: &BalanceUpdate) {
        let entry = self
            .balances
            .entry(update.asset.clone())
            .or_insert_with(|| AssetBalance {
                asset: update.asset.clone(),
                free: "0".into(),
                locked: "0".into(),
            });
        entry.free = add_decimal_strings(&entry.free, &update.delta);
    }

    fn apply_external_lock(&mut self, lock: &ExternalLockUpdate) {
        let entry = self
            .balances
            .entry(lock.asset.clone())
            .or_insert_with(|| AssetBalance {
                asset: lock.asset.clone(),
                free: "0".into(),
                locked: "0".into(),
            });
        entry.locked = add_decimal_strings(&entry.locked, &lock.delta);
    }

    fn apply_execution_report(&mut self, report: &ExecutionReport) {
        match report.order_status.as_str() {
            "FILLED" | "CANCELED" | "EXPIRED" | "REJECTED" => {
                self.open_orders.remove(&report.order_id);
            }
            _ => {
                self.open_orders.insert(report.order_id, report.clone());
            }
        }
    }
}

fn add_decimal_strings(lhs: &str, rhs: &str) -> String {
    let scale = decimal_scale(lhs).max(decimal_scale(rhs));
    let sum = to_scaled_i128(lhs, scale) + to_scaled_i128(rhs, scale);
    format_scaled(sum, scale)
}

fn decimal_scale(value: &str) -> usize {
    value.split('.').nth(1).map(str::len).unwrap_or(0)
}

fn to_scaled_i128(value: &str, scale: usize) -> i128 {
    let negative = value.starts_with('-');
    let normalized = value.trim_start_matches('-');
    let (whole, frac) = match normalized.split_once('.') {
        Some((whole, frac)) => (whole, frac),
        None => (normalized, ""),
    };
    let frac = format!("{frac:0<scale$}");
    let digits = format!("{whole}{}", &frac[..scale.min(frac.len())]);
    let parsed = digits.parse::<i128>().unwrap_or(0);
    if negative { -parsed } else { parsed }
}

fn format_scaled(value: i128, scale: usize) -> String {
    if scale == 0 {
        return value.to_string();
    }
    let negative = value < 0;
    let abs = value.unsigned_abs();
    let digits = abs.to_string();
    let (whole, frac) = if digits.len() <= scale {
        ("0", format!("{abs:0>scale$}"))
    } else {
        let split = digits.len() - scale;
        (&digits[..split], digits[split..].to_string())
    };
    if negative {
        format!("-{whole}.{frac}")
    } else {
        format!("{whole}.{frac}")
    }
}
