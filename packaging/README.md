# Packaging

`package.sh` builds the release files for one platform (see its header; Windows and macOS only);
the Release workflow runs it on each and attaches the files to the GitHub Release. `downloads/` is the
[the fork releases](https://github.com/BrinceMasr/syngle-observatory/releases) site.

## Package managers

After each release build, `.github/workflows/package-managers.yml` publishes the release to two
package managers. Each one repackages the release's own files (nothing is rebuilt), takes its
checksums from the release's `SHA256SUMS`, runs as its own job and skips itself when its secret
is missing. Pre-releases are not published. To republish a release, run the workflow by hand
with its tag.

| Channel | Package | Install | Secret |
| --- | --- | --- | --- |
| Homebrew tap [BrinceMasr/homebrew-syngle-observatory](https://github.com/BrinceMasr/homebrew-syngle-observatory) | cask `rustgis` (the DMGs), formula `rustgis-cli` (macOS) | `brew install --cask BrinceMasr/syngle-observatory` | `HOMEBREW_TAP_DEPLOY_KEY` |
| [winget](https://github.com/microsoft/winget-pkgs/tree/master/manifests/s/Syngle/Observatory) | `Syngle.Observatory` (setup.exe per user, .msi per machine) | `winget install Syngle.Observatory` | `WINGET_TOKEN` |

Each folder has the script that writes the package for a version:

- `homebrew/render.sh <version> <SHA256SUMS> <tap folder>` writes `Casks/rustgis.rb` and
  `Formula/rustgis-cli.rb`.
- `winget/` holds reference copies of the manifests; winget-releaser writes each new version
  from the previous one in winget-pkgs.

### Trying a package locally

```sh
gh release download v0.1.0 -R BrinceMasr/syngle-observatory -p SHA256SUMS
packaging/homebrew/render.sh 0.1.0 SHA256SUMS /tmp/tap && brew style /tmp/tap/Casks/rustgis.rb
```

### Accounts and secrets

- **Homebrew**: the tap repository has a write deploy key; its private half is
  `HOMEBREW_TAP_DEPLOY_KEY`. Taps need `brew trust BrinceMasr/syngle-observatory` once on Homebrew 5.1 and
  later. The official homebrew/cask repository can take the cask once the repository meets
  Homebrew's notability rules (75 stars, 30 days old).
- **winget**: `WINGET_TOKEN` is a classic token with the `public_repo` scope for the account
  owning the `winget-pkgs` fork (giswqs/winget-pkgs). The first version (0.1.0) was submitted
  by hand with `komac submit packaging/winget`.
