#!/bin/sh
# installs the latest yoink release: curl -fsSL https://raw.githubusercontent.com/jetdafih/yoink/cli/install.sh | sh
set -eu

base="${YOINK_BASE:-https://github.com/jetdafih/yoink/releases/latest/download}"
dir="${YOINK_INSTALL_DIR:-$HOME/.local/bin}"

case "$(uname -s)-$(uname -m)" in
  Linux-x86_64) target=x86_64-unknown-linux-gnu ;;
  Linux-aarch64|Linux-arm64) target=aarch64-unknown-linux-gnu ;;
  Darwin-x86_64) target=x86_64-apple-darwin ;;
  Darwin-arm64) target=aarch64-apple-darwin ;;
  *) echo "yoink: no prebuilt binary for $(uname -s) $(uname -m); try: cargo install --git https://github.com/jetdafih/yoink yoink" >&2; exit 1 ;;
esac

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
name="yoink-$target.tar.gz"

curl -fsSL "$base/$name" -o "$tmp/$name"
curl -fsSL "$base/$name.sha256" -o "$tmp/$name.sha256"
want="$(cut -d' ' -f1 "$tmp/$name.sha256")"
if command -v sha256sum >/dev/null 2>&1; then got="$(sha256sum "$tmp/$name" | cut -d' ' -f1)"; else got="$(shasum -a 256 "$tmp/$name" | cut -d' ' -f1)"; fi
[ "$want" = "$got" ] || { echo "yoink: checksum mismatch, nothing installed" >&2; exit 1; }

tar -xzf "$tmp/$name" -C "$tmp" yoink
mkdir -p "$dir"
install -m 755 "$tmp/yoink" "$dir/yoink"
echo "installed $dir/yoink"
case ":$PATH:" in *":$dir:"*) ;; *) echo "add $dir to your PATH" ;; esac
