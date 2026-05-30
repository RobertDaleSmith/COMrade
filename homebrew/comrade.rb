class Comrade < Formula
  desc "Modern serial, HID, and BLE device monitor for hardware hackers"
  homepage "https://github.com/RobertDaleSmith/COMrade"
  url "https://github.com/RobertDaleSmith/COMrade/archive/refs/tags/v0.1.0.tar.gz"
  sha256 "33d1b1715ddec6d5db071bd3712b972b07a6875c4f65412b41320508d1919b93"
  license "MIT"

  depends_on "rust" => :build

  def install
    cd "app" do
      system "cargo", "install", "--path", "crates/comrade-cli", "--root", prefix
    end
  end

  test do
    assert_match "comrade", shell_output("#{bin}/comrade --version")
  end
end
