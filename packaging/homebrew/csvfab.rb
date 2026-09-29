# Formula for the tap fabiochelly/homebrew-csvfab:  brew install fabiochelly/csvfab/csvfab
class Csvfab < Formula
  include Language::Python::Shebang

  desc "Desktop CSV editor: open, filter, clean and rewrite large CSV files in place"
  homepage "https://github.com/fabiochelly/csvfab"
  url "https://github.com/fabiochelly/csvfab/archive/refs/tags/v1.3.0.tar.gz"
  sha256 "01a3db028e74ee11770227e7e4626a9799faf38a5c8f68240ec87bc6d10d5c43"
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
    EOS
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/csvfab --version")
  end
end
