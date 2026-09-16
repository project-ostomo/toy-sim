fn main() -> anyhow::Result<()> {
    let path = std::env::args_os()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("provide output .ship path"))?;
    toy_sim_ships::armed_starter().save(path)
}
