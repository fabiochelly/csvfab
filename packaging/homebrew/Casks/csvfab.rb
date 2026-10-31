# Cask for the tap fabiochelly/homebrew-csvfab:  brew install --cask fabiochelly/csvfab/csvfab
# Installs csvfab.app in /Applications, assembled on this Mac by macos/build-app.sh
# from the release's program (one binary for Apple silicon and Intel), cleared of the
# download's quarantine flag and signed ad hoc. The formula of the same name is the
# command line alone.
cask "csvfab" do
  version "2.1.2"
  sha256 "8d02de587088f215fb4e62c75bc11febeae452e39f677579d530dd60d9b98060"

  url "https://github.com/fabiochelly/csvfab/releases/download/v#{version}/csvfab-#{version}-macos.tar.gz"
  name "csvfab"
  desc "Desktop CSV editor: open, filter, clean and rewrite large CSV files in place"
  homepage "https://github.com/fabiochelly/csvfab"

  depends_on macos: ">= :big_sur"
  # A cask can only conflict with a cask; the formula of the same name installs the
  # same command line, so the cask leaves the "csvfab" command to it (see caveats).

  installer script: {
    executable:   "csvfab/macos/build-app.sh",
    args:         ["#{staged_path}/csvfab", staged_path.to_s],
    must_succeed: true,
  }
  app "csvfab.app"

  zap trash: "~/Library/Application Support/csvfab"

  caveats <<~EOS
    A "csvfab" command for the terminal comes with the formula (brew install fabiochelly/csvfab/csvfab),
    or:  ln -s /Applications/csvfab.app/Contents/Resources/csvfab "$(brew --prefix)/bin/csvfab"

    csvfab is now offered in the Finder's "Open with" for CSV files. To make it the
    default: select a .csv, File › Get Info › Open with › csvfab › Change All…
    or, with duti (brew install duti):
      duti -s io.github.fabiochelly.csvfab public.comma-separated-values-text all
      duti -s io.github.fabiochelly.csvfab public.tab-separated-values-text all
  EOS
end
