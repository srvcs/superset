# srvcs-superset

The superset-test service of the srvcs.cloud distributed standard library.

Its single concern: **is `a` a superset of `b`?** It does no set logic of its
own. `a ⊇ b` is exactly `b ⊆ a`, so it delegates to
[`srvcs-subset`](https://github.com/srvcs/subset) with the operands **swapped**
and returns that service's boolean `result`:

```text
superset(a, b) = subset(a = b, b = a).result
```

For example, `superset({a: [1,2,3], b: [1,2]}) == true`.

## API

| Method | Path | Purpose |
| --- | --- | --- |
| `GET` | `/` | Service identity, concern, and dependency list |
| `POST` | `/` | Decide whether `a` is a superset of `b` |
| `GET` | `/healthz` `/readyz` `/metrics` `/openapi.json` | srvcs service standard surface |

```sh
curl -s -X POST localhost:8080/ -H 'content-type: application/json' -d '{"a": [1, 2, 3], "b": [1, 2]}'
# {"a":[1,2,3],"b":[1,2],"result":true}
```

Responses:

- `200 {"a": [...], "b": [...], "result": <bool>}` — evaluated.
- `422` — an element is not a valid integer, forwarded from `srvcs-subset`.
- `500` — a dependency returned a malformed result.
- `503` — the `srvcs-subset` dependency is unavailable.

## Dependencies

- [`srvcs-subset`](https://github.com/srvcs/subset)

This service is a pure orchestrator. It never calls `srvcs-isnumber` directly:
element validation propagates up the dependency graph from `srvcs-subset`, and
any `422` it raises is forwarded unchanged.

## Configuration

| Variable | Default | Purpose |
| --- | --- | --- |
| `SRVCS_BIND_ADDR` | `0.0.0.0:8080` | Bind address |
| `SRVCS_SUBSET_URL` | `http://127.0.0.1:8081` | Base URL of `srvcs-subset` |
| `SRVCS_ENV` | `development` | Environment label for logs |
| `RUST_LOG` | `info,tower_http=info` | Tracing filter |

## Local checks

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

Orchestration tests stand up a mock `srvcs-subset` in-process that **actually
computes** the subset relation from the request body (built from the real
`contains`/`subset`/`intersection` semantics), so the composition is genuinely
exercised (e.g. `superset([1,2,3], [1,2]) == true`). See
[`srvcs/platform`](https://github.com/srvcs/platform) for the shared standard.

> Note: the `cargoHash` in `flake.nix` is inherited from the template and must be
> refreshed with a `nix build` before the Nix gates pass.
