# Formula for the tap fabiochelly/homebrew-csvfab:  brew install fabiochelly/csvfab/csvfab
# The command line alone; the cask of the same name (brew install --cask …) is the
# application bundle for the Finder. Both cannot be installed at once.
class Csvfab < Formula
  include Language::Python::Shebang

  desc "Desktop CSV editor: open, filter, clean and rewrite large CSV files in place"
  homepage "https://github.com/fabiochelly/csvfab"
  url "https://github.com/fabiochelly/csvfab/archive/refs/tags/v1.5.0.tar.gz"
  sha256 "95dd75bacfeb95d50001a521f08d4350f99944da6f8c1150c63b6c6700d47680"
  license "MIT"

  depends_on "python@3.13"

  def install
    libexec.install "csvfab.py", "server.py", "viewer.htm", "papaparse.min.js", "icons", "ui"
    rewrite_shebang detected_python_shebang, libexec/"csvfab.py"
    bin.install_symlink libexec/"csvfab.py" => "csvfab"
  end

  def caveats
    <<~EOS
      csvfab opens its window in a Chromium-based browser (Chrome, Chromium, Brave or Edge).
      If none is installed:  brew install --cask google-chrome

      This is the command line only. For an application in /Applications that the Finder
      can open CSV files with (and make the default):  brew install --cask fabiochelly/csvfab/csvfab
    EOS
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/csvfab --version")
  end
end
