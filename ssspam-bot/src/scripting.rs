use std::sync::Arc;

use anyhow::anyhow;
use parking_lot::RwLock;
use rhai::{
    packages::{BasicMathPackage, Package},
    Engine,
};
use rhai_rand::RandomPackage;

pub fn interpret_rhai(source: &str) -> anyhow::Result<String> {
    let result = Arc::new(RwLock::new(String::new()));

    let mut engine = Engine::new();
    BasicMathPackage::new().register_into_engine(&mut engine);
    RandomPackage::new().register_into_engine(&mut engine);

    // Override action of 'print' function
    let logger = result.clone();
    engine.on_print(move |s| logger.write().push_str(s));

    engine.run(source).map_err(|e| anyhow!("{e:?}"))?;

    let ret = result.read().to_string();
    Ok(ret)
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_interpret_rhai() {
        assert_eq!(
            interpret_rhai(r#"for i in range(10, 21, 10) { print(`a p${i};`); }"#).unwrap(),
            "a p10;a p20;".to_owned(),
        );
    }
}
