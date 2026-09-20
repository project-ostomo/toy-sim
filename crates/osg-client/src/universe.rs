use osg_universe::universe::Universe;
use std::sync::{Arc, OnceLock};

pub(crate) fn shared_universe() -> anyhow::Result<Arc<Universe>> {
    static UNIVERSE: OnceLock<Result<Arc<Universe>, String>> = OnceLock::new();
    UNIVERSE
        .get_or_init(|| {
            Universe::bundled()
                .map(Arc::new)
                .map_err(|error| error.to_string())
        })
        .clone()
        .map_err(anyhow::Error::msg)
}
