# Bundle format

The contract between CI (`scripts/bundle.sh`, `.github/workflows/release.yml`) and the device (`carpi-update`, `carpi deploy`). Design and reasons: [DEPLOY.md](DEPLOY.md) section 1.

`format_version` is 1. Anything that changes a reader's behaviour bumps it; readers refuse a version they don't know.

## Version and channel

| Channel | Made by | `version` | Example |
|---|---|---|---|
| `stable` | tag `vX.Y.Z` | the tag without the `v` | `1.2.0` |
| `beta` | push to `main` | `beta-<YYYYMMDD>-<sha7>`, date of the pq35-carpi commit (UTC) | `beta-20261107-e7e3f15` |
| `dev` | any other branch, a PR, a local build | `dev-<sha7>` | `dev-e7e3f15` |

`version` only uses `[A-Za-z0-9._-]`, because it is also the directory name under `/opt/carpi/releases/`.

## Files

One bundle is three files. They are the release assets (stable and beta) and the workflow artifact `carpi-<version>-arm64` (every run):

```
carpi-<version>-arm64.tar.zst    the bundle
manifest.json                    copy of the one inside, so a device can check
manifest.json.minisig            version and signature before downloading the tarball
```

The `.minisig` is missing when the build had no signing key (forks before they set `MINISIGN_KEY`, PRs from forks).

## Tarball

zstd-compressed tar, GNU format. Only regular files: no directory entries, symlinks, hard links or devices. Names are relative, use `/`, have no `./` prefix and no `..` part. Parent directories are implied; create them 0755.

Order: `manifest.json`, then `manifest.json.minisig` if signed, then the files in `files[]` order. A streaming reader can verify the manifest before it reads anything else.

```
manifest.json
manifest.json.minisig
bin/carpid
bin/carpi
bin/carpi-update                    only once the crate exists
livi/out/bin/LIVI                   LIVI UI (livi-ui package)
livi/out/bin/livi-core
livi/out/bin/...                    the other LIVI binaries; livi/assets/linux/* its templates
bin/rnv3enc                         sdlink: encoder
bin/avlink                          sdlink: link to the Teensy
bin/wlcap                           sdlink: screen capture
share/config/carpid.toml            default config, reference only
share/systemd/user/carpid.service
share/systemd/user/livi.service
share/udev/99-carpi-uinput.rules
share/sdlink/run/...                sdlink pi/run, as-is (scripts, units, avsd.env)
firmware/teensy/sdemu.hex           only if the build had a Teensy image
```

`share/` holds defaults. The updater never copies them over user config (`~/.config/carpi`, `~/.config/LIVI`).

Every binary in `bin/` is an aarch64 ELF built on Debian Trixie (glibc 2.41).

## manifest.json

```json
{
  "format_version": 1,
  "version": "beta-20261107-e7e3f15",
  "channel": "beta",
  "arch": "arm64",
  "date": "2026-11-07T22:27:41Z",
  "commits": {
    "pq35-carpi": "e7e3f15c0a8d0d0b1f3f4d2a5c9e1b7a6d3c2b10",
    "livi": "1a4f05e73af7bdb189a30937ef65ea4c3a9009e4",
    "sdlink": "4044227cd05b518ed5097a385c1a762ab15816a6"
  },
  "min_os_profile": 1,
  "files": [
    { "path": "bin/carpi", "sha256": "<64 hex>", "size": 4321000, "mode": "0755" }
  ],
  "firmware": [
    { "target": "teensy", "path": "firmware/teensy/sdemu.hex", "version": "sdemu 0.4 (2026-11-05, touch, live stream, pool 1500)" }
  ]
}
```

| Field | Type | Meaning |
|---|---|---|
| `format_version` | integer | 1 |
| `version` | string | see above |
| `channel` | string | `stable`, `beta` or `dev` |
| `arch` | string | `arm64` |
| `date` | string | commit time of pq35-carpi HEAD, RFC 3339 UTC |
| `commits` | object | full 40-hex commit of each repo: `pq35-carpi`, `livi`, `sdlink` |
| `min_os_profile` | integer | lowest `PROFILE_VERSION` in `/etc/carpi-profile` this bundle runs on (`os/profile.sh`) |
| `files` | array | every file in the tarball except `manifest.json` and its `.minisig`, sorted by `path` |
| `files[].path` | string | as in the tarball |
| `files[].sha256` | string | lowercase hex |
| `files[].size` | integer | bytes |
| `files[].mode` | string | octal, `"0755"` or `"0644"`. Set this, not the tar header's mode |
| `firmware` | array | may be empty. Each image is also in `files` |
| `firmware[].target` | string | `teensy` for now |
| `firmware[].path` | string | the image, listed in `files` |
| `firmware[].version` | string | compared as a string; flash only when it differs from what's on the device |

Readers ignore fields they don't know. A tarball entry that isn't in `files` (or the two manifest files) is an error, and so is a listed file that's missing or has the wrong size or hash.

## Signature

[minisign](https://jedisct1.github.io/minisign/) over the exact bytes of `manifest.json`, prehashed (minisign's default since 0.10). The trusted comment is `carpi <version> <channel>`; check it matches the manifest.

The files are covered through their hashes in the manifest, so verify order is: signature on `manifest.json`, then every file against it.

Key: the secret key is the repo secret `MINISIGN_KEY`, the whole secret key file made with `minisign -G -W` (no password; CI can't type one). Each fork makes its own. The device trusts only a public key it already has (`/etc/carpi/minisign.pub`, put there at first install); it never trusts a key that arrives in a bundle.

An unsigned bundle (no `.minisig`) is only for direct deploy; whether to accept one is the updater's call (suggested: only with an explicit flag).
