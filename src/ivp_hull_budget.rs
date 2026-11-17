//! Numeric object-hull motion budget in owned1010f960. The hull-manager
//! threshold callbacks, rebase lists and broadphase graph remain separate.
//! This component does not schedule native probe times from a recording.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Budget {
    pub time: f64,
    pub rates: [f32; 2],
    pub accumulated: [f32; 2],
    pub predicted: f32,
    pub threshold: f32,
}

impl Budget {
    ///1010ee60 changes only the stored double clock, not motion/entry keys.
    pub(crate) fn rebase_time(&mut self,shift:f64){self.time-=shift;}
    /// Current accumulated motion used by10127f00 when renewing pair entries.
    /// The retained clock need not equal the environment's current time.
    pub(crate) fn motion_at(&self, now: f64) -> f32 {
        ((now-self.time)*f64::from(self.rates[0])+f64::from(self.accumulated[0])) as f32
    }
    pub(crate) fn advance(&mut self, now: f64, dt: f32, linear: f32, angular: f32) -> bool {
        // Native stores the combined speed before its safety multiplication.
        let combined = angular + linear;
        let rate = combined * 1.0000100135803223_f32;
        let elapsed = now - self.time;
        self.time = now;
        for i in 0..2 {
            self.accumulated[i] = (elapsed * f64::from(self.rates[i])
                + f64::from(self.accumulated[i])) as f32;
        }
        self.rates = [rate, linear];
        // Both stores matter: retaining the unrounded product here changes
        // native recheck budgets by one float step in the captured calls.
        let projected = dt * rate;
        self.predicted = projected + self.accumulated[0];
        self.threshold - self.predicted < 0.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn state(v: &serde_json::Value) -> Budget {
        let pair = |key: &str| std::array::from_fn(|i| v[key][i].as_f64().unwrap() as f32);
        Budget { time: v["time"].as_f64().unwrap(), rates: pair("rates"),
            accumulated: pair("accumulated"), predicted: v["predicted"].as_f64().unwrap() as f32,
            threshold: v["threshold"].as_f64().unwrap() as f32 }
    }
    #[test]
    fn native_motion_budgets_and_recheck_insertions_match_every_observed_call() {
        let data: serde_json::Value = serde_json::from_str(include_str!("../tools/fixtures/native-ivp-hull-budget.json")).unwrap();
        let mut queued = 0;
        for c in data["cases"].as_array().unwrap() {
            let mut source = state(&c["before"]);
            let input = |key: &str| c[key].as_f64().unwrap() as f32;
            let request = source.advance(c["time"].as_f64().unwrap(), input("dt"), input("linear"), input("angular"));
            assert_eq!(source, state(&c["after"]), "{} step{}", c["model"], c["step"]);
            assert_eq!(request, c["queued_after"].as_u64().unwrap() > c["queued_before"].as_u64().unwrap());
            queued += usize::from(request);
        }
        assert_eq!(data["cases"].as_array().unwrap().len(), 397);
        assert_eq!(queued, 54);
    }
    #[test]
    fn equal_threshold_does_not_recheck_and_elapsed_motion_uses_previous_rates() {
        let mut budget = Budget { time: 3., rates: [2., 1.], accumulated: [0.25, 0.125], predicted: 0., threshold: 1.25 };
        assert!(!budget.advance(3.5, 0., 0., 0.));
        assert_eq!(budget.accumulated, [1.25, 0.625]);
        assert_eq!(budget.rates, [0., 0.]);
        budget.threshold = f32::from_bits(budget.threshold.to_bits()-1);
        assert!(budget.advance(3.5, 0., 0., 0.));
    }
}
