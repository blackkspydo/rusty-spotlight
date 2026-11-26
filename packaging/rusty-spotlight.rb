# The Homebrew cask. scripts/release.sh fills in the version and checksum and
# puts the result in github.com/blackkspydo/homebrew-tap as Casks/rusty-spotlight.rb.
cask "rusty-spotlight" do
  version "@VERSION@"
  sha256 "@SHA256@"

  url "https://github.com/blackkspydo/rusty-spotlight/releases/download/v#{version}/Rusty-Spotlight-#{version}.zip"
  name "Rusty Spotlight"
  desc "Spotlight replacement written in Rust"
  homepage "https://blackkspydo.github.io/rusty-spotlight/"

  livecheck do
    url :url
    strategy :github_latest
  end

  depends_on macos: :sonoma

  app "Rusty Spotlight.app"

  uninstall quit: "io.github.blackkspydo.rusty-spotlight"

  zap trash: [
    "~/.config/rusty-spotlight",
    "~/Library/Application Support/rusty-spotlight",
    "~/Library/Logs/rusty-spotlight",
  ]

  caveats <<~EOS
    Open Rusty Spotlight once from Applications; it then opens at login.
    Option-Space opens it until you choose another shortcut in Settings.

    If you installed an earlier copy from source, remove it first with
    scripts/uninstall.sh, so the shortcut isn't taken twice.
  EOS
end
