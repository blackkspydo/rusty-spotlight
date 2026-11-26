#!/bin/sh
# Build the copy of Rusty Spotlight that is given to others: a universal app,
# signed with a Developer ID, notarized by Apple, zipped into dist/.
#
#   scripts/release.sh            build, sign, notarize; leaves dist/Rusty-Spotlight-<version>.zip
#   scripts/release.sh --publish  the same, then a GitHub release and the Homebrew cask
#   scripts/release.sh --dry-run  build and sign for this Mac only, to try the bundle; nothing is sent anywhere
#
# The version is the workspace's, in Cargo.toml. Needed once on this Mac:
#   1. A "Developer ID Application" certificate in the keychain
#      (Xcode > Settings > Accounts > Manage Certificates > +).
#   2. Notary credentials, stored in the keychain under the name "rusty-spotlight":
#      xcrun notarytool store-credentials rusty-spotlight --apple-id <Apple ID> --team-id <team>
#      (it asks for an app-specific password from account.apple.com).
# SIGN_IDENTITY and NOTARY_PROFILE override what is looked up.
set -eu
cd "$(dirname "$0")/.."

MODE="${1:-}"
ID="io.github.blackkspydo.rusty-spotlight"
REPO="blackkspydo/rusty-spotlight"
TAP="blackkspydo/homebrew-tap"
PROFILE="${NOTARY_PROFILE:-rusty-spotlight}"
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
APP="dist/Rusty Spotlight.app"
ZIP="dist/Rusty-Spotlight-$VERSION.zip"

if [ "$MODE" = "--dry-run" ]; then
  IDENTITY="-"
else
  IDENTITY="${SIGN_IDENTITY:-$(security find-identity -v -p codesigning | sed -n 's/.*"\(Developer ID Application: .*\)"/\1/p' | head -1)}"
  if [ -z "$IDENTITY" ]; then
    echo "No Developer ID Application certificate in the keychain. See the top of this script." >&2
    exit 1
  fi
  if [ -n "$(git status --porcelain)" ]; then
    echo "Commit or stash your changes first: a release is built from a commit." >&2
    exit 1
  fi
fi

# Apple silicon and Intel in one binary.
rustup target add aarch64-apple-darwin x86_64-apple-darwin >/dev/null
cargo build --release --target aarch64-apple-darwin
cargo build --release --target x86_64-apple-darwin
rm -rf dist && mkdir -p dist
lipo -create -output dist/rusty-spotlight \
  target/aarch64-apple-darwin/release/rusty-spotlight \
  target/x86_64-apple-darwin/release/rusty-spotlight
scripts/bundle.sh "$APP" dist/rusty-spotlight "$ID"
rm dist/rusty-spotlight

if [ "$MODE" = "--dry-run" ]; then
  codesign --force --sign - "$APP"
  ditto -c -k --keepParent "$APP" "$ZIP"
  echo "Dry run: $APP (signed for this Mac only), $ZIP"
  exit 0
fi

# The hardened runtime and a secure timestamp are what notarization asks for.
codesign --force --options runtime --timestamp --sign "$IDENTITY" "$APP"
codesign --verify --strict --verbose=2 "$APP"

# Apple checks the zip; the ticket is then attached to the app, so it opens
# without a warning even offline, and the zip is made again around it.
ditto -c -k --keepParent "$APP" "$ZIP"
xcrun notarytool submit "$ZIP" --keychain-profile "$PROFILE" --wait
xcrun stapler staple "$APP"
rm "$ZIP"
ditto -c -k --keepParent "$APP" "$ZIP"
spctl --assess --type execute --verbose=2 "$APP"
SHA=$(shasum -a 256 "$ZIP" | cut -d' ' -f1)
echo "Built $ZIP"
echo "sha256 $SHA"

[ "$MODE" = "--publish" ] || exit 0

# The GitHub release the cask downloads from.
export GH_TOKEN="${GH_TOKEN:-$(gh auth token --user blackkspydo)}"
git tag "v$VERSION"
git push origin "v$VERSION"
gh release create "v$VERSION" "$ZIP" --repo "$REPO" --title "Rusty Spotlight $VERSION" \
  --notes "What changed: https://blackkspydo.github.io/rusty-spotlight/changelog.html

Install with Homebrew:

    brew install --cask blackkspydo/tap/rusty-spotlight"

# The cask, in the tap.
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
git clone -q "https://x-access-token:$GH_TOKEN@github.com/$TAP.git" "$WORK/tap"
mkdir -p "$WORK/tap/Casks"
sed -e "s/@VERSION@/$VERSION/" -e "s/@SHA256@/$SHA/" packaging/rusty-spotlight.rb > "$WORK/tap/Casks/rusty-spotlight.rb"
git -C "$WORK/tap" add Casks/rusty-spotlight.rb
git -C "$WORK/tap" -c user.name="$(git config user.name)" -c user.email="$(git config user.email)" \
  commit -q -m "rusty-spotlight $VERSION"
git -C "$WORK/tap" push -q origin HEAD
echo "Published. brew install --cask blackkspydo/tap/rusty-spotlight"
