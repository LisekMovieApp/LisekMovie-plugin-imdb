# LisekMovie-plugin-imdb

IMDb metadata provider extension for [LisekMovie](https://github.com/LisekMovieApp/LisekMovie-app), the self-hosted video-on-demand server.

| | |
|---|---|
| Provider key | `imdb` |
| Covers | Movies, series |
| Status | Basic — works, parts still missing |
| API key | None |
| Fix match ids | `tt0133093`, `imdb:tt0133093`, `https://www.imdb.com/title/tt0133093/` |

## What it does

IMDb's public title suggestion endpoint (`v3.sg.media-imdb.com/suggestion`): title search, lookup by IMDb id, year, kind and poster. No plot, genres or ratings — pair it with OMDb or TMDB for those. The endpoint is unofficial and may change.

## How it plugs in

The crate implements `lisek_plugin_api::MetadataProvider` and is compiled into `lisek-server` as a Cargo path dependency — there is no runtime plugin loading. Clone it **next to** `LisekMovie-app`:

```
LisekMovie/
  LisekMovie-app/
  LisekMovie-plugin-imdb/
  ...
```

The server lists the provider under **Admin → Plugins** (disabled by default). Turn it on, add the key if it needs one, and set its priority — globally, or per library (for example AniList or aniDB first in an anime library). `install-linux.sh` and `update-linux.sh` clone and update it with the other modules.

## Development

```bash
cargo clippy --all-targets -- -D warnings
cargo test
```

Tests run against a local `wiremock` server; nothing calls the real service.

## License

[MIT](LICENSE), like LisekMovie itself.
