use std::marker::PhantomData;

use bevy::{
    asset::{AssetLoader, LoadContext, io::Reader},
    prelude::*,
};
use serde::de::DeserializeOwned;

/// Asset loader for TOML star system configuration files.
#[derive(Default)]
pub struct TomlAssetLoader<T: Asset> {
    ext: [&'static str; 1],
    _ph: PhantomData<T>,
}

impl<T: Asset + DeserializeOwned> TomlAssetLoader<T> {
    pub fn new(ext: &'static str) -> Self {
        Self {
            ext: [ext],
            _ph: PhantomData,
        }
    }
}

impl<T: Asset + DeserializeOwned> AssetLoader for TomlAssetLoader<T> {
    type Asset = T;
    type Settings = ();
    type Error = anyhow::Error;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &(),
        _load_context: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        let cfg = toml::from_slice::<T>(&bytes)?;
        Ok(cfg)
    }

    fn extensions(&self) -> &[&str] {
        &self.ext
    }
}
