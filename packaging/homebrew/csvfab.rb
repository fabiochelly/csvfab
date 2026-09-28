# Formula for the tap fabiochelly/homebrew-csvfab:  brew install fabiochelly/csvfab/csvfab
class Csvfab < Formula
  include Language::Python::Shebang

  desc "Desktop CSV editor: open, filter, clean and rewrite large CSV files in place"
  homepage "https://github.com/fabiochelly/csvfab"
  url "https://github.com/fabiochelly/csvfab/archive/refs/tags/v1.0.1.tar.gz"
  sha256 "7a932edaee7f7a96ef173deec3e0894446c6b183fd84a7882390a71dfa27925f"
  license "MIT"

  depends_on "python@3.13"

  def install
    libexec.install "csvfab", "server.py", "viewer.htm", "papaparse.min.js", "icons"
    rewrite_shebang detected_python_shebang, libexec/"csvfab"
    bin.install_symlink libexec/"csvfab"
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
