# Cask for the tap fabiochelly/homebrew-csvfab:  brew install --cask fabiochelly/csvfab/csvfab
# Installs csvfab.app in /Applications, built on this Mac from the release sources by
# macos/build-app.sh (an AppleScript applet: nothing downloaded as an app, so no
# Gatekeeper quarantine). The formula of the same name is the command line alone.
cask "csvfab" do
  version "1.5.0"
  sha256 "95dd75bacfeb95d50001a521f08d4350f99944da6f8c1150c63b6c6700d47680"

  url "https://github.com/fabiochelly/csvfab/archive/refs/tags/v#{version}.tar.gz"
  name "csvfab"
  desc "Desktop CSV editor: open, filter, clean and rewrite large CSV files in place"
  homepage "https://github.com/fabiochelly/csvfab"

  depends_on formula: "python@3.13"
  conflicts_with formula: "csvfab"

  installer script: {
    executable: "csvfab-#{version}/macos/build-app.sh",
    args:       ["#{staged_path}/csvfab-#{version}", staged_path.to_s],
    must_succeed: true,
  }
  app "csvfab.app"
  binary "#{appdir}/csvfab.app/Contents/Resources/csvfab/csvfab.py", target: "csvfab"

  zap trash: "~/Library/Application Support/csvfab"

  caveats <<~EOS
    csvfab opens its window in a Chromium-based browser (Chrome, Chromium, Brave or Edge).
    If none is installed:  brew install --cask google-chrome

    csvfab is now offered in the Finder's "Open with" for CSV files. To make it the
    default: select a .csv, File › Get Info › Open with › csvfab › Change All…
    or, with duti (brew install duti):
      duti -s io.github.fabiochelly.csvfab public.comma-separated-values-text all
      duti -s io.github.fabiochelly.csvfab public.tab-separated-values-text all
  EOS
end
