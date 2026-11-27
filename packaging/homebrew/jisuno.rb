# Cask template. Fill __VERSION__ and __SHA256__ for each release (sha256 of the
# notarized, stapled zip attached to the GitHub release), then copy into the tap.
cask "jisuno" do
  version "__VERSION__"
  sha256 "__SHA256__"

  url "https://github.com/kolisachint/jisuno/releases/download/v#{version}/Jisuno-#{version}-universal.zip"
  name "Jisuno"
  desc "Local, offline voice-to-text and voice commands for macOS"
  homepage "https://github.com/kolisachint/jisuno"

  depends_on macos: ">= :ventura"

  app "Jisuno.app"

  zap trash: [
    "~/Library/Application Support/jisuno",
    "~/.config/jisuno",
    "~/Library/Caches/jisuno",
    "~/Library/Logs/Jisuno",
    "~/Library/Preferences/dev.kolisachint.jisuno.plist",
    "~/Library/Saved Application State/dev.kolisachint.jisuno.savedState",
  ]
end
