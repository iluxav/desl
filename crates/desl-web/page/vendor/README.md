# Vendored for desl web

Copied unchanged from npm, so `cargo build` stays the only build (web-spec.md "Architecture").
Each tarball's sha512 was checked against the registry's `dist.integrity` before copying.

| File | Package | Licence | Integrity of the tarball |
| --- | --- | --- | --- |
| `xterm.mjs`, `xterm.css` | `@xterm/xterm` 6.0.0 (`lib/xterm.mjs`, `css/xterm.css`) | MIT, `LICENSE-xterm` | `sha512-TQwDdQGtwwDt+2cgKDLn0IRaSxYu1tSUjgKarSDkUM0ZNiSRXFpjxEsvc/Zgc5kq5omJ+V0a8/kIM2WD3sMOYg==` |
| `addon-fit.mjs` | `@xterm/addon-fit` 0.11.0 (`lib/addon-fit.mjs`) | MIT, `LICENSE-addon-fit` | `sha512-jYcgT6xtVYhnhgxh3QgYDnnNMYTcf8ElbxxFzX0IZo+vabQqSPAjC3c1wJrKB5E19VwQei89QCiZZP86DCPF7g==` |

To update one: download the new tarball from `https://registry.npmjs.org/<package>/-/<name>-<version>.tgz`,
check its sha512 against `dist.integrity` in `https://registry.npmjs.org/<package>/<version>`, copy
the same files, and change this table.
