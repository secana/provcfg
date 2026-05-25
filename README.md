# provcfg

A Rust config loader that tracks where each value came from.

`provcfg` (provenance config) layers configuration sources (compiled-in
defaults, files, environment variables, CLI flags) into one struct, and keeps
the *provenance* of every leaf field: which source set the active value, and
which earlier sources it overrode.

## Choosing between provcfg and `config`

For most projects the [`config`](https://crates.io/crates/config) crate is the
better fit. It is mature, supports more formats, and is less ceremony if all
you need is the merged result.

Use `provcfg` when you need to know which source set each value, for example:

- a settings page that labels each field "from file" / "from env" / "default";
- diagnosing "why is this value set to that?" across layered sources;
- an admin UI that renders the effective config together with its origin.

If you never ask "where did this come from?", prefer `config`.

## What it does

Derive `Configurable` on a plain config struct. `Config::build` returns a
companion `*Prov` struct whose every leaf is a `ValueHistory`: the value paired
with the `Source` it came from.

```rust
use provcfg::{Category, Config, Configurable};

#[derive(Configurable, serde::Deserialize, Clone, Default)]
struct Settings {
    host: String,
    port: u16,
}

// `APP_HOST=db.internal` is set in the environment; `APP_PORT` is not.
let settings = Config::new()
    .add_env("APP")
    .build::<SettingsProv>()
    .unwrap();

assert_eq!(settings.host.value(), "db.internal");
assert_eq!(settings.host.source().category(), Category::Env);

// `port` was set by nobody, so it falls back to the compiled-in default.
assert_eq!(settings.port.value(), &0);
assert_eq!(settings.port.source().category(), Category::Default);
```

## Layering sources

Sources are applied in the order they are added. A later source overrides an
earlier one for any leaf it sets. Unset leaves keep the earlier value, and the
overridden values stay in each leaf's history.

```rust
// `from_cli` is a `SettingsPartial` (every field an `Option`) produced by a
// CLI parser; here it carries only `port = 9090`.
let settings = Config::new()
    .add_toml_str("defaults.toml", "host = \"localhost\"\nport = 8080")
    .add_env("APP")        // environment has APP_HOST=db.internal
    .add_cli(from_cli)     // command line had --port 9090
    .build::<SettingsProv>()
    .unwrap();

assert_eq!(settings.host.value(), "db.internal");           // env overrode the file
assert_eq!(settings.host.source().category(), Category::Env);
assert_eq!(settings.port.value(), &9090);                   // cli overrode the file
assert_eq!(settings.port.source().category(), Category::Cli);
```

## Inspecting provenance

`sources_map` returns a flat `dotted.path -> Category` map for a settings UI:

```rust
let map = settings.sources_map();
assert_eq!(map.get("host"), Some(&Category::Env));
assert_eq!(map.get("port"), Some(&Category::Cli));
```

`walk_leaves` visits every leaf with its value, category, and a secret flag, so
an effective-config view can redact sensitive fields:

```rust
settings.walk_leaves("", &mut |path, value, category, is_secret| {
    let shown = if is_secret {
        "<redacted>".to_string()
    } else {
        serde_json::to_string(value).unwrap()
    };
    println!("{path} = {shown}  ({})", category.as_str());
});
```

## Field attributes

```rust
#[derive(Configurable, serde::Deserialize, Clone, Default)]
struct Settings {
    #[configurable(nested)]               // recurse, tracking provenance per leaf
    database: DatabaseConfig,
    #[configurable(secret)]               // redact in UIs (ValueHistory::is_secret)
    api_token: String,
    #[configurable(rename = "LOG_LEVEL")] // rename source key (verbatim for file formats; lowercase alias for env)
    log_level: String,
    #[configurable(env_list)]             // accept "a,b,c" for a Vec<String>
    allowed_hosts: Vec<String>,
    #[configurable(skip)]                 // keep on the struct, hide from provcfg
    runtime_cache: Cache,
}
```

## Custom sources

To support a format the built-ins don't cover (XML, a database, a secret
store), implement the [`Source`] trait. Most sources build something that
implements `serde::Deserializer` (a `serde_json::Value` works fine) and erase
it through `erased_serde`:

```rust
use provcfg::erased_serde;
use provcfg::{Category, Config, Source};

struct MapSource { name: String, data: serde_json::Value }

impl Source for MapSource {
    fn name(&self) -> &str { &self.name }
    fn category(&self) -> Category { Category::Custom("map") }
    fn deserialize(
        &self,
        seed: &mut dyn for<'de> FnMut(
            &mut dyn erased_serde::Deserializer<'de>,
        ) -> Result<(), erased_serde::Error>,
    ) -> Result<(), erased_serde::Error> {
        let mut erased = <dyn erased_serde::Deserializer>::erase(&self.data);
        seed(&mut erased)
    }
}

let cfg = Config::new()
    .add_source(MapSource { name: "demo".into(), data: serde_json::json!({ "host": "x" }) })
    .build::<SettingsProv>()
    .unwrap();
```

Two contract notes:

- `Source::deserialize` is called synchronously from `Config::build`. I/O and
  async work belong in the source's constructor: fetch on `new`, store the
  materialized data, hand it out on demand.
- Convert your own error into `erased_serde::Error` via
  `<erased_serde::Error as serde::de::Error>::custom(my_error)`. `Config::build`
  wraps that in [`Error::Deserialize`] with the source's `name()`.

See the `Source` trait's rustdoc for the full doctested example.

[`Source`]: https://docs.rs/provcfg/latest/provcfg/trait.Source.html
[`Error::Deserialize`]: https://docs.rs/provcfg/latest/provcfg/enum.Error.html

## Cargo features

Only `env` is enabled by default; opt into the file and CLI sources you need.

| Feature       | Enables                            | Extra dependency |
|---------------|------------------------------------|------------------|
| `env`         | `EnvSource` / `add_env` (default)  | `serde_json`     |
| `json`        | `JsonStr` / `add_json_*`           | `serde_json`     |
| `toml`        | `TomlStr` / `add_toml_*`           | `toml`           |
| `cli`         | `CliSource` / `add_cli`            | `serde_json`     |
| `clap-derive` | `ClapArgs` derive (`provcfg-clap`) | `clap`           |

```toml
[dependencies]
provcfg = { version = "0.1", features = ["toml", "cli"] }
```

## clap integration

With the `clap-derive` feature, derive `ClapArgs` alongside `Configurable` to
generate a clap-compatible args struct that flows straight into a CLI source.
See the `provcfg-clap` crate.

## License

Licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or
  <https://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or
  <https://opensource.org/licenses/MIT>)

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall
be dual-licensed as above, without any additional terms or conditions.
