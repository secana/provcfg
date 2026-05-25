use provcfg::Configurable;
use provcfg_clap::ClapArgs;

#[derive(serde::Deserialize, serde::Serialize, Clone, Default, Configurable, ClapArgs)]
#[configurable(clap_prefx = "app")]
struct CliCfg {
    host: String,
}

fn main() {}
