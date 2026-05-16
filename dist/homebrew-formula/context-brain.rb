# Homebrew formula for context-brain.
#
# This file is a TEMPLATE. To publish it:
#   1. Create a public repo named `homebrew-context-brain` (the `homebrew-` prefix is required).
#   2. Copy this file to `Formula/context-brain.rb` in that repo.
#   3. Replace the {{VERSION}} and {{SHA256_*}} placeholders with values from a
#      released tag (the release.yml workflow uploads .tar.gz.sha256 files alongside
#      each binary — read those).
#   4. Push. Users can then run:
#        brew tap yarik1987k/context-brain
#        brew install context-brain
#
# The release workflow could be extended later to auto-bump this formula on each
# tag via a PAT-authenticated commit to the tap repo.
class ContextBrain < Formula
  desc "Local-first MCP server that cuts LLM token bills 60-90%"
  homepage "https://github.com/yarik1987k/contexa-brain-mcp"
  version "{{VERSION}}" # e.g. "0.1.0"
  license "MIT"

  on_macos do
    on_arm do
      url "https://github.com/yarik1987k/contexa-brain-mcp/releases/download/v#{version}/context-brain-aarch64-apple-darwin.tar.gz"
      sha256 "{{SHA256_DARWIN_ARM64}}"
    end
    on_intel do
      url "https://github.com/yarik1987k/contexa-brain-mcp/releases/download/v#{version}/context-brain-x86_64-apple-darwin.tar.gz"
      sha256 "{{SHA256_DARWIN_X86_64}}"
    end
  end

  on_linux do
    on_intel do
      url "https://github.com/yarik1987k/contexa-brain-mcp/releases/download/v#{version}/context-brain-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "{{SHA256_LINUX_X86_64}}"
    end
  end

  def install
    bin.install "context-brain"
  end

  test do
    assert_match "context-brain", shell_output("#{bin}/context-brain --version")
  end
end
