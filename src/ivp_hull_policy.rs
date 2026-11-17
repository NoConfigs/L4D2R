//! Owned101170a0/10117140 policy and10127f00's pair threshold renewal.
//! Geometry/candidate queries and manager-list ownership are separate. Inputs
//! come from the caller's cores/budgets; this never supplies recorded times.
use crate::ivp_hull_budget::Budget;

#[derive(Clone, Copy, Debug)]
pub(crate) struct Body {
    pub radius: f32,
    pub linear: f32,
    pub angular: f32,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Policy {
    pub lookahead: f32,
    pub radius_factor: f32,
    pub minimum: f32,
    pub maximum: f32,
    pub minimum_time: f32,
}
impl Default for Policy {
    fn default() -> Self {
        Self { lookahead: 0.5, radius_factor: 0.9, minimum: 0.8,
            maximum: 10., minimum_time: 0.06 }
    }
}
impl Policy {
    pub(crate) fn allowances(self, dt: f32, bodies: [Body; 2]) -> [f32; 2] {
        // Intermediate native float stores are part of the policy contract.
        let first = (bodies[0].angular+bodies[0].linear)+1e-10_f32;
        let second = (bodies[1].angular+bodies[1].linear)+1e-10_f32;
        let speed = second+first;
        let radius = if bodies[0].radius < bodies[1].radius { bodies[0].radius } else { bodies[1].radius };
        let mut distance = self.lookahead*speed;
        let size_limit = self.radius_factor*radius;
        if size_limit < distance { distance = size_limit; }
        if distance < self.minimum { distance = self.minimum; }
        if self.maximum < distance { distance = self.maximum; }
        let predicted = dt*speed;
        distance -= predicted;
        let time_limit = self.minimum_time*speed;
        if distance < time_limit { distance = time_limit; }
        // Native biases the allocations in this order; replacing this with
        // a symmetric speed ratio changes stationary/unequal-speed pairs.
        let first_share = second*0.2_f32+first;
        let second_share = first_share*0.18_f32+second;
        let inverse = 1./(second_share+first_share);
        [distance*first_share*inverse, distance*second_share*inverse]
    }

    /// Apply after the pair's native-equivalent geometry query completes.
    pub(crate) fn thresholds(self, now: f64, dt: f32, bodies: [Body; 2], budgets: &[Budget; 2]) -> [f32; 2] {
        let allowances = self.allowances(dt, bodies);
        std::array::from_fn(|i| budgets[i].motion_at(now)+allowances[i])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn bodies(v: &serde_json::Value) -> [Body; 2] {
        std::array::from_fn(|i| Body { radius: v[i*3].as_f64().unwrap() as f32,
            linear: v[i*3+1].as_f64().unwrap() as f32, angular: v[i*3+2].as_f64().unwrap() as f32 })
    }
    fn pair(v: &serde_json::Value) -> [f32; 2] { std::array::from_fn(|i| v[i].as_f64().unwrap() as f32) }
    fn settings(v: &serde_json::Value) -> Policy {
        Policy { lookahead: v[0].as_f64().unwrap() as f32, radius_factor: v[1].as_f64().unwrap() as f32,
            minimum: v[2].as_f64().unwrap() as f32, maximum: v[3].as_f64().unwrap() as f32,
            minimum_time: v[4].as_f64().unwrap() as f32 }
    }
    #[test]
    fn owned_constructor_and_all_distance_allowances_match_exactly() {
        let data: serde_json::Value = serde_json::from_str(include_str!("../tools/fixtures/native-ivp-hull-policy.json")).unwrap();
        let policy = Policy::default();
        assert_eq!([policy.lookahead, policy.radius_factor, policy.minimum, policy.maximum, policy.minimum_time],
            std::array::from_fn(|i| data["hull_policy_defaults"][i].as_f64().unwrap() as f32));
        let cases = data["hull_policy_controls"].as_array().unwrap();
        for c in cases {
            assert_eq!(policy.allowances(c["dt"].as_f64().unwrap() as f32, bodies(&c["objects"])), pair(&c["expected"]), "control{}", c["id"]);
        }
        assert_eq!(cases.len(), 960);
    }
    #[test]
    fn actual_distance_callbacks_renew_both_object_thresholds_exactly() {
        let mut types = [0; 3];
        for capture in [include_str!("../tools/fixtures/native-ivp-hull-manager.json"), include_str!("../tools/fixtures/native-ivp-hull-manager-wake.json")] {
         let data: serde_json::Value = serde_json::from_str(capture).unwrap();
         for c in data["cases"].as_array().unwrap() {
            let head = &c["before"]["entries"][0];
            let method = head["methods_rva"][1].as_u64().unwrap();
            let role = match method { 0x1280a0 => 0, 0x112490 => 1, 0x10d500 => 2, _ => panic!("unknown manager callback") };
            types[role] += 1;
            if role != 0 { continue; }
            let p = &head["pair"];
            let budgets = std::array::from_fn(|i| Budget { time: p["budget_times"][i].as_f64().unwrap(),
                rates: [p["rates"][i].as_f64().unwrap() as f32, 0.],
                accumulated: [p["accumulated"][i].as_f64().unwrap() as f32, 0.], predicted: 0., threshold: 0. });
            let keys = settings(&p["settings"]).thresholds(p["time"].as_f64().unwrap(), p["dt"].as_f64().unwrap() as f32, bodies(&p["objects"]), &budgets);
            let after = c["after"]["entries"].as_array().unwrap().iter().filter(|e| e["methods_rva"][1] == 0x1280a0).collect::<Vec<_>>();
            assert_eq!(after.len(), 1, "controlled pair must remain registered");
            assert_eq!(keys, pair(&after[0]["pair"]["keys"]), "{} step{}", c["model"], c["step"]);
            let side = p["own_side"].as_u64().unwrap() as usize;
            assert_eq!(keys[side], after[0]["key"].as_f64().unwrap() as f32);
         }
        }
        // Preserve all observed dispatches; other listener types are explicit
        // remaining callback work, not discarded failed distance comparisons.
        assert_eq!(types, [37, 23, 26]);
    }
}
