# Homebrew formula for the Intent daemon and CLI. The app is the `intent-app` cask.
#   brew tap pranav5255/intent https://github.com/Pranav5255/Intent
#   brew install --HEAD intent      # until a tagged release exists
#   intentctl enable
class Intent < Formula
  desc "Local-first companion that remembers what you were working on"
  homepage "https://github.com/Pranav5255/Intent"
  license "MIT"
  head "https://github.com/Pranav5255/Intent.git", branch: "main"

  depends_on "rust" => :build

  def install
    system "cargo", "install", *std_cargo_args(path: "crates/intentd")
    system "cargo", "install", *std_cargo_args(path: "crates/intentctl")
    (share/"intent/shell").install "shell/intent.bash", "shell/intent.zsh"
    (share/"intent").install "packaging/blocked-domains.yaml.example"
  end

  def caveats
    <<~EOS
      Start Intent at login and add the zsh/bash hook:
        intentctl enable
      Stop it again with:
        intentctl disable

      Intent sees which app you use without any permission. To record window
      titles too, allow intentd under System Settings > Privacy & Security >
      Screen Recording. Everything stays on this Mac.
    EOS
  end

  test do
    port = free_port
    (testpath/"cfg").mkpath
    (testpath/"data").mkpath
    env = {
      "HOME"                      => testpath.to_s,
      "XDG_CONFIG_HOME"           => (testpath/"cfg").to_s,
      "XDG_DATA_HOME"             => (testpath/"data").to_s,
      "INTENT_BIND"               => "127.0.0.1:#{port}",
      "INTENT_DISABLE_COLLECTORS" => "1",
    }
    pid = spawn(env, bin/"intentd")
    begin
      sleep 3
      assert_match "ok", shell_output("curl -s http://127.0.0.1:#{port}/healthz")
      assert_match "pause", shell_output("#{bin}/intentctl --help")
    ensure
      Process.kill("TERM", pid)
      Process.wait(pid)
    end
  end
end
