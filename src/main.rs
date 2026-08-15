use std::collections::HashMap;

use crate::feature_functions::new_feature_map;
pub mod feature_functions;

fn main() {
    let feature_map: HashMap<String, f64> = new_feature_map();
    for (feature, value) in &feature_map {
        println!("{feature:} {value}");
    }
}
