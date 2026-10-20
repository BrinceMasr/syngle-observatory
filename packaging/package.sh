#!/usr/bin/env bash
# Packages release builds of rustgis and rustgis-cli for one target into dist/.
#
#   packaging/package.sh <version> [rust-target]
#
# macOS: a .dmg holding Syngle Observatory.app, plus a .tar.gz of the CLI
# (signed and notarized when the MACOS_SIGN_IDENTITY / APPLE_API_* variables are set).
# Windows (Git Bash): a portable .zip, an NSIS setup.exe (per user), a WiX .msi (all users) and
# an .msix for the Microsoft Store. The installers need makensis and the wix .NET tool with the
# WixToolset.UI.wixext extension (`wix extension add -g WixToolset.UI.wixext/<wix version>`).
# Build first with `cargo build --release --locked -p rustgis -p rustgis-cli [--target T]`.
set -euo pipefail

version="${1:?usage: package.sh <version> [rust-target]}"
version="${version#v}"
target="${2:-}"
root="$(cd "$(dirname "$0")/.." && pwd)"
bin="$root/target/${target:+$target/}release"
dist="$root/dist"
mkdir -p "$dist"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

case "$(uname -s)" in
  Darwin*) os=macos ;;
  MINGW* | MSYS* | CYGWIN*) os=windows ;;
  *) echo "unsupported OS: $(uname -s)" >&2; exit 1 ;;
esac
arch="${target%%-*}"
[ -n "$arch" ] || arch="$(uname -m)"
case "$arch" in
  x86_64 | amd64) arch=x86_64 ;;
  aarch64 | arm64) arch=aarch64 ;;
esac
exe=""
[ "$os" = windows ] && exe=".exe"
for b in rustgis rustgis-cli; do
  [ -f "$bin/$b$exe" ] || { echo "missing $bin/$b$exe: build it first" >&2; exit 1; }
done
logo() { "$bin/rustgis-cli$exe" logo "$1" --size "$2"; }
name="syngle-observatory-$version-$os-$arch"

case "$os" in
  macos)
    app="$work/Syngle Observatory.app"
    mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
    cp "$bin/rustgis" "$app/Contents/MacOS/rustgis"
    # Bundle versions must be dot-separated numbers: drop a pre-release suffix (0.2.0-rc.1).
    sed "s/@VERSION@/${version%%-*}/g" "$root/packaging/macos/Info.plist" > "$app/Contents/Info.plist"
    iconset="$work/rustgis.iconset"
    mkdir -p "$iconset"
    for s in 16 32 128 256 512; do
      logo "$iconset/icon_${s}x${s}.png" "$s"
      logo "$iconset/icon_${s}x${s}@2x.png" "$((s * 2))"
    done
    iconutil -c icns "$iconset" -o "$app/Contents/Resources/rustgis.icns"
    # Signing: with MACOS_SIGN_IDENTITY (a Developer ID Application identity in the keychain)
    # the app and CLI get a hardened-runtime signature; without it, an ad-hoc one, which
    # Apple Silicon needs to launch anything. With APPLE_API_KEY_PATH, APPLE_API_KEY_ID and
    # APPLE_API_ISSUER as well, the app, the DMG and the CLI are notarized and the app and the
    # DMG are stapled.
    identity="${MACOS_SIGN_IDENTITY:-}"
    sign() {
      if [ -n "$identity" ]; then
        codesign --force --options runtime --timestamp --sign "$identity" "$@"
      else
        codesign --force --sign - "$@"
      fi
    }
    notarize() {
      [ -n "$identity" ] && [ -n "${APPLE_API_KEY_PATH:-}" ] || return 0
      xcrun notarytool submit "$1" --key "$APPLE_API_KEY_PATH" --key-id "$APPLE_API_KEY_ID" \
        --issuer "$APPLE_API_ISSUER" --wait --timeout 30m
    }
    # Stapling fetches the ticket from Apple's CloudKit, which drops connections now and then
    # ("The network connection was lost", error 68): try a few times before failing the build.
    staple() {
      for attempt in 1 2 3 4 5 6 7 8; do
        xcrun stapler staple "$1" && return 0
        [ "$attempt" -lt 8 ] && sleep $((attempt * 15))
      done
      return 1
    }
    sign "$app"
    # Staple the app itself, not only the DMG: Homebrew and people dragging it to Applications
    # copy the app out of the DMG, and an app without its own ticket opens only if Gatekeeper
    # reaches Apple's servers at first launch (#89). Stapling adds a file outside the signed
    # contents, so the signature stays valid.
    if [ -n "$identity" ] && [ -n "${APPLE_API_KEY_PATH:-}" ]; then
      ditto -c -k --keepParent "$app" "$work/Syngle Observatory.app.zip"
      notarize "$work/Syngle Observatory.app.zip"
      staple "$app"
    fi

    stage="$work/dmg"
    mkdir -p "$stage"
    cp -R "$app" "$stage/"
    ln -s /Applications "$stage/Applications"
    hdiutil create -volname "Syngle Observatory $version" -srcfolder "$stage" -ov -format UDZO "$dist/$name.dmg"
    if [ -n "$identity" ]; then
      sign "$dist/$name.dmg"
      if notarize "$dist/$name.dmg" && [ -n "${APPLE_API_KEY_PATH:-}" ]; then
        staple "$dist/$name.dmg"
      fi
    fi

    d="$work/rustgis-cli-$version-$os-$arch"
    mkdir -p "$d"
    cp "$bin/rustgis-cli" "$root/LICENSE" "$d/"
    sign "$d/rustgis-cli"
    # A bare executable cannot be stapled: notarizing it registers the ticket that Gatekeeper
    # looks up online.
    if [ -n "$identity" ]; then
      ditto -c -k "$d/rustgis-cli" "$work/rustgis-cli.zip"
      notarize "$work/rustgis-cli.zip"
    fi
    tar -C "$work" -czf "$dist/rustgis-cli-$version-$os-$arch.tar.gz" "$(basename "$d")"
    ;;

  windows)
    # Portable zip: unzip anywhere and run, no installer or admin rights.
    d="$work/$name"
    mkdir -p "$d"
    cp "$bin/rustgis.exe" "$bin/rustgis-cli.exe" "$root/LICENSE" "$root/README.md" "$d/"
    sed "s/@VERSION@/$version/g" "$root/packaging/windows/README-portable.txt" > "$d/README.txt"
    (cd "$work" && 7z a -tzip -bso0 "$dist/$name-portable.zip" "$name")

    # Installers, as GeoLibre ships: setup.exe (NSIS) installs for the current user without admin
    # rights, the .msi (WiX) for all users. Both add a Start menu shortcut, put rustgis-cli on the
    # PATH and open .rgis projects. Wizard images are the logo on white, drawn by rustgis-cli.
    case "$arch" in x86_64) instarch=x64 ;; aarch64) instarch=arm64 ;; *) instarch="$arch" ;; esac
    s="$work/installer"
    mkdir -p "$s"
    cp "$bin/rustgis.exe" "$bin/rustgis-cli.exe" "$root/LICENSE" "$root/packaging/windows/path.ps1" "$s/"
    "$bin/rustgis-cli.exe" logo "$s/rustgis.ico"
    "$bin/rustgis-cli.exe" logo "$s/sidebar.bmp" --size 128 --canvas 164x314
    "$bin/rustgis-cli.exe" logo "$s/banner.bmp" --size 44 --canvas 493x58 --at 440,7
    "$bin/rustgis-cli.exe" logo "$s/dialog.bmp" --size 120 --canvas 493x312 --at 22,96
    w() { cygpath -w "$1"; }
    # A missing tool fails a CI build (the release must carry every package) and is skipped
    # locally.
    missing() {
      if [ -n "${CI:-}" ]; then echo "$1 not found" >&2; exit 1; fi
      echo "$1 not found: skipping the $2" >&2
    }

    makensis="$(command -v makensis.exe || command -v makensis || true)"
    [ -n "$makensis" ] || [ ! -x "/c/Program Files (x86)/NSIS/makensis.exe" ] ||
      makensis="/c/Program Files (x86)/NSIS/makensis.exe"
    if [ -n "$makensis" ]; then
      # VIProductVersion wants four numbers: drop a pre-release suffix (0.2.0-rc.1 -> 0.2.0.0).
      MSYS2_ARG_CONV_EXCL='*' "$makensis" -V2 -DVERSION="$version" -DVERSION4="${version%%-*}.0" \
        -DSTAGE="$(w "$s")" -DICON="$(w "$s/rustgis.ico")" -DSIDEBAR="$(w "$s/sidebar.bmp")" \
        -DOUTFILE="$(w "$dist/$name-setup.exe")" "$(w "$root/packaging/windows/rustgis.nsi")"
    else
      missing makensis setup.exe
    fi

    wix="$(command -v wix.exe || command -v wix || true)"
    if [ -n "$wix" ]; then
      # The license dialog shows RTF: LICENSE with RTF's special characters escaped.
      {
        printf '{\\rtf1\\ansi\\deff0{\\fonttbl{\\f0 Segoe UI;}}\\f0\\fs16\n'
        tr -d '\r' < "$root/LICENSE" | sed -e 's/[\\{}]/\\&/g' -e 's/$/\\par/'
        printf '}\n'
      } > "$s/LICENSE.rtf"
      # MSI versions are three numbers: drop a pre-release suffix.
      MSYS2_ARG_CONV_EXCL='*' "$wix" build -arch "$instarch" -ext WixToolset.UI.wixext \
        -d Version="${version%%-*}" -d Stage="$(w "$s")" -d Icon="$(w "$s/rustgis.ico")" \
        -d License="$(w "$s/LICENSE.rtf")" -d Banner="$(w "$s/banner.bmp")" -d Dialog="$(w "$s/dialog.bmp")" \
        -o "$(w "$dist/$name.msi")" "$(w "$root/packaging/windows/rustgis.wxs")"
      rm -f "$dist/$name.wixpdb"
    else
      missing wix .msi
    fi

    # MSIX package for the Microsoft Store, which signs it on ingestion. The identity must
    # match the product reserved in Partner Center (Product identity); override it for a
    # self-signed sideload build.
    m="$work/msix"
    mkdir -p "$m/Assets"
    cp "$bin/rustgis.exe" "$bin/rustgis-cli.exe" "$m/"
    logo "$m/Assets/Square44x44Logo.png" 44
    logo "$m/Assets/Square150x150Logo.png" 150
    logo "$m/Assets/StoreLogo.png" 50
    # A value for the manifest: XML-escaped, then escaped for a sed replacement.
    xmlval() { printf '%s' "$1" | sed -e 's/&/\&amp;/g; s/</\&lt;/g; s/>/\&gt;/g; s/"/\&quot;/g' -e 's/[\\|&]/\\&/g'; }
    # MSIX versions are four numbers: drop a pre-release suffix (0.2.0-rc.1 -> 0.2.0.0).
    sed -e "s|@VERSION@|${version%%-*}.0|g" -e "s|@ARCH@|$instarch|g" \
      -e "s|@NAME@|$(xmlval "${MSIX_NAME:-Syngle.Observatory}")|g" \
      -e "s|@PUBLISHER@|$(xmlval "${MSIX_PUBLISHER:-CN=E6AE8172-DC4F-4F79-844B-9D84204BF95A}")|g" \
      -e "s|@PUBLISHER_DISPLAY_NAME@|$(xmlval "${MSIX_PUBLISHER_DISPLAY_NAME:-Syngle}")|g" \
      "$root/packaging/windows/AppxManifest.xml" > "$m/AppxManifest.xml"
    makeappx="$(command -v makeappx.exe || true)"
    if [ -z "$makeappx" ]; then
      # The host's own build of the SDK tools, x64 or ARM64.
      case "$(uname -m)" in aarch64 | arm64) sdkarch=arm64 ;; *) sdkarch=x64 ;; esac
      makeappx="$(ls -d "/c/Program Files (x86)/Windows Kits/10/bin/"*/"$sdkarch"/makeappx.exe 2>/dev/null | sort -V | tail -1)"
    fi
    [ -n "$makeappx" ] || { echo "makeappx.exe not found: install the Windows SDK" >&2; exit 1; }
    MSYS2_ARG_CONV_EXCL='*' "$makeappx" pack /d "$(cygpath -w "$m")" /p "$(cygpath -w "$dist/$name.msix")" /o
    ;;
esac

ls -l "$dist"
