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

  # Model files: setup.rs currently stores them under
  # ~/Library/Application Support/voicetools/models (pre-rename path). The jisuno
  # location is not decided yet; add it here once it is, e.g.
  # "~/Library/Application Support/jisuno".
  zap trash: [
    "~/.config/jisuno",
    "~/Library/Caches/jisuno",
    "~/Library/Preferences/dev.kolisachint.jisuno.plist",
    "~/Library/Saved Application State/dev.kolisachint.jisuno.savedState",
  ]
end
