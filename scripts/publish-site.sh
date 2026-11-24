#!/bin/sh
# Publish site/ to the gh-pages branch, which GitHub Pages serves at
# https://blackkspydo.github.io/rusty-spotlight/. The demo video, poster
# and icon are copied in from assets/. The branch holds only the latest
# copy of the site (it is replaced each time), so the video is not kept
# twice in history. Run after changing site/ or assets/.
set -eu
cd "$(dirname "$0")/.."

entry() { # mode, file on disk, name on the site
  printf '%s blob %s\t%s\n' "$1" "$(git hash-object -w "$2")" "$3"
}
TREE=$({
  entry 100644 site/index.html index.html
  entry 100644 site/changelog.html changelog.html
  entry 100644 assets/demo.mp4 demo.mp4
  entry 100644 assets/demo-poster.png demo-poster.png
  entry 100644 assets/AppIcon.png AppIcon.png
  entry 100644 /dev/null .nojekyll
} | git mktree)
COMMIT=$(git commit-tree "$TREE" -m "Publish site from $(git rev-parse --short HEAD)")
git push --force origin "$COMMIT:refs/heads/gh-pages"
echo "Published. https://blackkspydo.github.io/rusty-spotlight/"
