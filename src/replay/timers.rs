//! Timer model (static only): 66 ms per TicksLeft, distinct from 30 Hz turns.
//! Queue nesting and start-at-head behavior remain hypotheses pending a dump.
use serde_json::{json, Value};

pub(super) fn new(seconds: u32) -> Value {
    let ticks = u64::from(seconds) * 1000 / 66;
    if ticks == 0 {
        Value::Null
    } else {
        json!({"StartSeconds":seconds,"TicksLeft":ticks})
    }
}
pub(super) fn remaining(timer: &Value) -> u64 {
    timer.get("TicksLeft").and_then(Value::as_u64).unwrap_or(0)
}
fn decrement(timer: &mut Value, elapsed: u64) {
    if !timer.is_object() {
        return;
    }
    let left = remaining(timer).saturating_sub(elapsed);
    if left == 0 {
        *timer = Value::Null;
    } else {
        timer["TicksLeft"] = json!(left);
    }
}
pub(super) fn advance_state(state: &mut Value, elapsed: u64) {
    if let Some(timer) = state.get_mut("Timer") {
        decrement(timer, elapsed);
    }
}
pub(super) fn advance_object(value: &mut Value, elapsed: u64) {
    let kind = value["ID"].as_u64().unwrap_or(0) / 100000;
    if kind == 13 {
        if let Some(timer) = value.get_mut("Timer") {
            decrement(timer, elapsed);
        }
    } else if kind == 21 {
        if let Some(timer) = value.get_mut("ConstructionTimer") {
            decrement(timer, elapsed);
        }
    } else if kind == 32 {
        if let Some(orders) = value.get_mut("Orders").and_then(Value::as_array_mut) {
            for order in orders.iter_mut() {
                if let Some(timer) = order.get_mut("Timer") {
                    decrement(timer, elapsed);
                }
            }
            // An expired order timer must stay an object, never null.
            super::orders::normalize_empty_timers(orders);
        }
    }
    let Some(list) = value
        .get_mut("ProductionList")
        .and_then(Value::as_object_mut)
    else {
        return;
    };
    // Preserve unrelated and unrecognized JSON rather than reinterpreting it.
    if !list.get("Productions").is_some_and(Value::is_array) {
        return;
    }
    if list
        .get("FinishedProductions")
        .is_some_and(|v| !v.is_array())
    {
        return;
    }
    let mut active = list
        .remove("Productions")
        .unwrap()
        .as_array()
        .unwrap()
        .clone();
    let mut finished = list
        .remove("FinishedProductions")
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default();
    let mut budget = elapsed;
    while let Some(head) = active.first_mut() {
        if !head.is_object() {
            break;
        } // Unknown saved nesting is not a new format to guess.
        let left = remaining(&head["Timer"]);
        if left > budget {
            decrement(&mut head["Timer"], budget);
            break;
        }
        budget -= left;
        let mut entry = active.remove(0);
        entry["Timer"] = Value::Null;
        finished.push(entry);
        // The next head begins now; any excess elapsed time advances it too.
    }
    list.insert("Productions".into(), json!(active));
    list.insert("FinishedProductions".into(), json!(finished));
}
pub(super) fn has_running(value: &Value) -> bool {
    remaining(&value["Timer"]) > 0
        || remaining(&value["ConstructionTimer"]) > 0
        || value["Orders"]
            .as_array()
            .is_some_and(|orders| orders.iter().any(|o| remaining(&o["Timer"]) > 0))
        || value["ProductionList"]["Productions"]
            .as_array()
            .is_some_and(|items| !items.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn timer_units_and_serial_queue_advance() {
        assert_eq!(new(1), json!({"StartSeconds":1,"TicksLeft":15}));
        let mut animal = json!({"ID":1300000,"Fed":true,"Timer":new(2)});
        advance_object(&mut animal, 15);
        assert_eq!(remaining(&animal["Timer"]), 15);
        advance_object(&mut animal, 15);
        assert_eq!(animal["Timer"], Value::Null);
        assert_eq!(animal["Fed"], true);
        let mut building = json!({"ID":600000,"ProductionList":{"Productions":[{"ID":2400000,"Timer":new(1)},{"ID":2400001,"Timer":new(1)}],"FinishedProductions":[]}});
        advance_object(&mut building, 20);
        assert_eq!(
            building["ProductionList"]["FinishedProductions"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            remaining(&building["ProductionList"]["Productions"][0]["Timer"]),
            10
        );
        advance_object(&mut building, 10);
        assert!(building["ProductionList"]["Productions"]
            .as_array()
            .unwrap()
            .is_empty());
        assert_eq!(
            building["ProductionList"]["FinishedProductions"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
    }
}
