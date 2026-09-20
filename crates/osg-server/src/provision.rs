use anyhow::{Result, ensure};
use ed25519_dalek::SigningKey;
use osg_model::Id;
use std::{fs::OpenOptions, io::Write, path::Path};

pub fn demo(directory: &Path) -> Result<()> {
    std::fs::create_dir_all(directory)?;
    let names = ["server.toml", "client-a.toml", "client-b.toml"];
    ensure!(
        names.iter().all(|name| !directory.join(name).exists()),
        "configuration files already exist"
    );
    let server = SigningKey::from_bytes(&rand::random());
    let accounts = [
        (Id::new(), SigningKey::from_bytes(&rand::random())),
        (Id::new(), SigningKey::from_bytes(&rand::random())),
    ];
    let mut config = format!(
        "listen = \"127.0.0.1:23000\"\nserver_secret = \"{}\"\n",
        hex(&server.to_bytes())
    );
    for (id, key) in &accounts {
        config.push_str(&format!(
            "\n[[accounts]]\nid = \"{id}\"\npublic_key = \"{}\"\n",
            hex(&key.verifying_key().to_bytes())
        ));
    }
    write(&directory.join(names[0]), &config)?;
    for (name, (id, key)) in names[1..].iter().zip(&accounts) {
        write(
            &directory.join(name),
            &format!(
                "address = \"127.0.0.1:23000\"\nserver_public_key = \"{}\"\naccount = \"{id}\"\naccount_secret = \"{}\"\n",
                hex(&server.verifying_key().to_bytes()),
                hex(&key.to_bytes())
            ),
        )?;
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn write(path: &Path, text: &str) -> Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)?.write_all(text.as_bytes())?;
    Ok(())
}
