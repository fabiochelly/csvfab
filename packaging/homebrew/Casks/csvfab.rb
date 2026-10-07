# Cask for the tap fabiochelly/homebrew-csvfab:  brew install --cask fabiochelly/csvfab/csvfab
# Installs csvfab.app in /Applications, built on this Mac from the release sources by
# macos/build-app.sh (an AppleScript applet: nothing downloaded as an app, so no
# Gatekeeper quarantine). The formula of the same name is the command line alone.
cask "csvfab" do
  version "1.20.0"
  sha256 "6e5a805319f8c7dfddceda6757c4f67c6e885f6a54c5e1a53bd8495a9599be4a"

  url "https://github.com/fabiochelly/csvfab/archive/refs/tags/v#{version}.tar.gz"
  name "csvfab"
  desc "Desktop CSV editor: open, filter, clean and rewrite large CSV files in place"
  homepage "https://github.com/fabiochelly/csvfab"

  depends_on formula: "python@3.13"
  # A cask can only conflict with a cask; the formula of the same name installs the
  # same command line, so the cask leaves the "csvfab" command to it (see caveats).

  installer script: {
    executable: "csvfab-#{version}/macos/build-app.sh",
    args:       ["#{staged_path}/csvfab-#{version}", staged_path.to_s],
    must_succeed: true,
  }
  app "csvfab.app"

  zap trash: "~/Library/Application Support/csvfab"

  caveats <<~EOS
    csvfab opens its window in a Chromium-based browser (Chrome, Chromium, Brave or Edge).
    If none is installed:  brew install --cask google-chrome

    A "csvfab" command for the terminal comes with the formula (brew install fabiochelly/csvfab/csvfab),
    or:  ln -s /Applications/csvfab.app/Contents/Resources/csvfab/csvfab.py "$(brew --prefix)/bin/csvfab"

    csvfab is now offered in the Finder's "Open with" for CSV files. To make it the
    default: select a .csv, File › Get Info › Open with › csvfab › Change All…
    or, with duti (brew install duti):
      duti -s io.github.fabiochelly.csvfab public.comma-separated-values-text all
      duti -s io.github.fabiochelly.csvfab public.tab-separated-values-text all
  EOS
end
