cask "comrade-app" do
  version "0.1.0"
  sha256 "4c4b033324456dc95c004cc201f6d92c5871ae5a31de3350154d86a22961f68a"

  url "https://github.com/RobertDaleSmith/COMrade/releases/download/v#{version}/COMrade_#{version}_aarch64.dmg"
  name "COMrade"
  desc "Serial, HID, and BLE device monitor"
  homepage "https://github.com/RobertDaleSmith/COMrade"

  depends_on formula: "robertdalesmith/comrade/comrade"

  app "COMrade.app"

  zap trash: [
    "~/Library/Application Support/com.comrade.serial-monitor",
    "~/Library/Caches/com.comrade.serial-monitor",
  ]
end
