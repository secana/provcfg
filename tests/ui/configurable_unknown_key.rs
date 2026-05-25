use provcfg::Configurable;

#[derive(serde::Deserialize, Clone, Default, Configurable)]
struct BadConfig {
    #[configurable(renmae = "HOST")]
    host: String,
}

fn main() {}
