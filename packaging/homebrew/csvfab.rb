# Formula for the tap fabiochelly/homebrew-csvfab:  brew install fabiochelly/csvfab/csvfab
# The command line alone, built from the sources; the cask of the same name
# (brew install --cask …) is the application bundle for the Finder.
class Csvfab < Formula
  desc "Desktop CSV editor: open, filter, clean and rewrite large CSV files in place"
  homepage "https://github.com/fabiochelly/csvfab"
  url "https://github.com/fabiochelly/csvfab/archive/refs/tags/v2.1.1.tar.gz"
  sha256 "d27f4154621d8c41a69268f03772a8007ef4653bd8e1a8b3af8b5eda080e4e5b"
  license "MIT"

  depends_on "rust" => :build

  def install
    system "cargo", "install", *std_cargo_args(path: "app")
  end

  def caveats
    <<~EOS
      This is the command line only. For an application in /Applications that the Finder
      can open CSV files with (and make the default):  brew install --cask fabiochelly/csvfab/csvfab
    EOS
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/csvfab --version")
  end
end
