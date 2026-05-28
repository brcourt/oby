class Oby < Formula
  desc "Live, per-agent activity feed for Claude Code"
  homepage "https://github.com/brcourt/oby"
  url "https://github.com/brcourt/oby/archive/refs/tags/v0.1.0.tar.gz"
  sha256 "d9668a8fe077354f076f2483c30b633ecc1e2322e6fa7b0dbe1b50d6d1ce68d1"
  license "MIT"
  head "https://github.com/brcourt/oby.git", branch: "main"

  depends_on "rust" => :build

  def install
    # Build all three binaries from the workspace and stage them into the
    # Homebrew prefix. The `oby` binary is the wrapper-daemon; oby-hook is
    # invoked by Claude Code via ~/.claude/settings.json; oby-tee is spawned
    # inside the rewritten Bash command line at runtime.
    system "cargo", "install", "--no-track", "--locked", "--root", prefix, "--path", "crates/oby"
    system "cargo", "install", "--no-track", "--locked", "--root", prefix, "--path", "crates/oby-hook"
    system "cargo", "install", "--no-track", "--locked", "--root", prefix, "--path", "crates/oby-tee"
  end

  def caveats
    <<~EOS
      Next steps:
        1. Tell Claude Code about the hook:
             oby install
           This writes Pre/Post/Failure tool-use hooks (Bash, Read) and a
           SubagentStop entry to ~/.claude/settings.json.
        2. Launch claude inside the wrapper:
             oby claude
           Press Ctrl-G to toggle between claude and the activity feed.

      Plain `claude` (no wrapper) still works — oby-hook env-gates itself
      and no-ops outside a wrapped session.
    EOS
  end

  test do
    # `--version` exercises each binary's clap setup without touching any
    # external resources (no socket, no PATH lookups).
    assert_match version.to_s, shell_output("#{bin}/oby --version")
    assert_match version.to_s, shell_output("#{bin}/oby-hook --version")
    assert_match version.to_s, shell_output("#{bin}/oby-tee --version")
  end
end
