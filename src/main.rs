use anyhow::{Context, Result, bail};
use confuse::{
    api::Server,
    store::{DEFAULT_PROJECT, Store},
};
use std::{env, path::PathBuf};

fn main() -> Result<()> {
    let mut args = env::args().skip(1);
    let command = args.next();
    let override_root = args.next().map(PathBuf::from);
    let data_dir = data_dir(override_root)?;
    match command.as_deref() {
        Some("desktop") => {
            let home = PathBuf::from(env::var_os("HOME").context("HOME is unavailable")?);
            let store = Store::open(data_dir.join("confuse.sqlite"))?;
            let runtime = tokio::runtime::Runtime::new()?;
            let result = runtime.block_on(confuse::desktop::run(store, home));
            runtime.shutdown_timeout(std::time::Duration::from_secs(2));
            result?;
        }
        Some("key") => {
            let store = Store::open(data_dir.join("confuse.sqlite"))?;
            println!(
                "{}",
                serde_json::to_string(&store.create_key(DEFAULT_PROJECT, "sdk")?)?
            );
        }
        Some("serve") => {
            let store = Store::open(data_dir.join("confuse.sqlite"))?;
            let runtime = tokio::runtime::Runtime::new()?;
            runtime.block_on(async move {
                let port = env::var("CONFUSE_PORT")
                    .ok()
                    .map(|value| value.parse::<u16>())
                    .transpose()?
                    .unwrap_or(store.endpoint_port()?);
                let server = Server::start(store, port).await?;
                println!("http://{}", server.address);
                tokio::signal::ctrl_c().await?;
                server.stop().await;
                anyhow::Ok(())
            })?;
        }
        _ => bail!("Usage: confuse serve [data-dir] | confuse key [data-dir]"),
    }
    Ok(())
}

fn data_dir(override_root: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(root) = override_root.or_else(|| env::var_os("CONFUSE_DATA_DIR").map(PathBuf::from))
    {
        return Ok(root);
    }
    let home = env::var_os("HOME").context("HOME is unavailable")?;
    Ok(PathBuf::from(home).join("Library/Application Support/Confuse-Zero"))
}
