# Forking rnse-carpi

How someone builds their own carpi on top of this one: their own changes, their own releases, their own cars updating from them.

A device talks to one repo's releases only (`[update] repo` in `~/.config/carpi/carpid.toml`). So a fork of rnse-carpi with its own GitHub releases is a complete product: nothing goes through `DSparks156x`.

## The repos

| Repo | What | Fork it when |
|---|---|---|
| `rnse-carpi` | carpid, the tools, `os/` (install, first boot), CI, the bundle | always: it's the one that builds and publishes releases |
| `rnse-livi` | the LIVI fork (`livi/` submodule, branch `carpi`) | you change LIVI |
| `rnse-sdlink` | SDLink and its firmware (`sdlink/` submodule) | you change SDLink |
| `rnse-carpi-image` | the setup page that writes a card | you want a page that installs your releases (see "Cards" below) |

The submodules point at `DSparks156x/rnse-livi` and `DSparks156x/rnse-sdlink` by full URL. A fork that doesn't change them keeps them as they are; CI clones them (they're public). A fork that does: fork that repo too, point `.gitmodules` at it (`git config -f .gitmodules submodule.livi.url ...`, then `git submodule sync`) and commit the new submodule commit in rnse-carpi.

## Releases from a fork

1. Fork `rnse-carpi` on GitHub.
2. Actions tab: turn workflows on (GitHub leaves them off on forks).
3. Optional, signing (below). Without it, releases are unsigned and say so.
4. Push to `main`. The `release` workflow tests, builds the arm64 bundle and publishes a pre-release `beta-<date>-<sha7>` in your repo. A `vX.Y.Z` tag makes a stable release. Nothing in the workflow names `DSparks156x`.

The bundle builds on `ubuntu-24.04-arm` runners: free on a public repo; on a private one they're billed minutes.

## Signing

Releases can be signed with [minisign](https://jedisct1.github.io/minisign/); the device then refuses an update that isn't signed by the key it knows. Each repo has its own key, and nobody else's key is ever needed.

1. `minisign -G -W -s minisign.key -p minisign.pub` (`-W`: no password, since CI can't type one).
2. Repo Settings, Secrets and variables, Actions: new secret `MINISIGN_KEY`, the whole contents of `minisign.key`. Or `gh secret set MINISIGN_KEY -R you/rnse-carpi < minisign.key`.
3. Keep `minisign.key` somewhere safe and off the repo. Lose it and devices installed with the matching public key need it replaced by hand.

Nothing else: CI recreates the public key from the secret and puts it in each signed bundle (`share/keys/minisign.pub`). `os/install.sh` installs it as `/etc/carpi/minisign.pub` on a device's first install. `carpi-update` never takes a key from a bundle after that.

## Cards

A fresh card that installs your releases from the start:

- **The setup page** (https://cowpowersystems.com/carpi/). Release step, advanced: the release repo, `owner/name` or a GitHub link; the page lists that repo's releases. A link that fills it in: `https://cowpowersystems.com/carpi/?repo=you/rnse-carpi`. The page writes it into the card's `carpi/setup.json` as `release.repo`: first boot installs from it and carpid updates from it (`[update] repo`). Hosting your own copy of the page is optional: fork `rnse-carpi-image` and change `REPO` in `src/lib/build.js`, the default.
- **`cargo xtask image`** (DEPLOY.md, section 7): writes a card with your own bundle on it, no page needed.

First boot downloads your release, installs it and trusts your key (if it's signed). From then on the device updates from your repo.

## Moving a device that runs ours to your fork

The device knows our key, so your releases (signed by yours, or unsigned) are refused until it's told otherwise. On the Pi:

```bash
sudo install -m 0644 minisign.pub /etc/carpi/minisign.pub   # your public key; or sudo rm it for checksums only
sed -i 's|^repo = .*|repo = "you/rnse-carpi"|' ~/.config/carpi/carpid.toml
systemctl --user restart carpid
```

Then Update on the head unit.

## Developing without releases

No GitHub, no key: build on a PC and push straight to a Pi on the bench.

- `bash scripts/cross-build.sh` (Docker) builds everything for arm64 and the bundle into `dist/`.
- `cargo xtask deploy --file dist/carpi-dev-<sha>-arm64.tar.zst` copies it to the Pi and applies it with `--allow-unsigned`, so it works on a device that has a key too.

## Licenses

rnse-carpi and rnse-sdlink are PolyForm Noncommercial 1.0.0 (`LICENSE.md`; `carpi-proto` is MIT/Apache). rnse-livi is LIVI's GPL-3.0-or-later. A fork keeps them: noncommercial use only for the first two, source shared for LIVI changes you distribute.
