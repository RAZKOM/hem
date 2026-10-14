# hem demo, rendered to docs/demo.gif by VHS (https://github.com/charmbracelet/vhs).
#
# CI re-renders it on every version tag (.github/workflows/demo.yml).
# To render locally, from the repo root:
#   cargo build --release
#   cargo run --release --example genlog -- --lines 200000 --out demo.log
#   PATH="$PWD/target/release:$PATH" vhs docs/demo.tape

Output docs/demo.gif

Require hem

Set Shell "bash"
Set FontSize 15
Set Width 1280
Set Height 760
Set Padding 12
Set Framerate 24
Set Theme "TokyoNight"
Set TypingSpeed 70ms
Set CursorBlink false

Hide
Type "export COLORTERM=truecolor && clear"
Enter
Sleep 300ms
Show

Type "hem demo.log"
Sleep 500ms
Enter
Sleep 2.5s

# Jump straight to the errors
Type "e"
Sleep 1.2s
Type "e"
Sleep 1.2s

# Search, then step through the matches
Type "/"
Sleep 400ms
Type "timeout"
Sleep 400ms
Enter
Sleep 1.2s
Type "n"
Sleep 900ms
Type "n"
Sleep 1s

# Hide everything below WARN
Type "l"
Sleep 300ms
Type "l"
Sleep 300ms
Type "l"
Sleep 1.8s

# Line detail: fields and surrounding context
Enter
Sleep 2.5s
Escape
Sleep 600ms

# Templates view, rarest lines first
Type "t"
Sleep 2s
Type "r"
Sleep 2.5s
Escape
Sleep 600ms

# Command palette
Type ":"
Sleep 500ms
Type "templ"
Sleep 1.5s
Escape
Sleep 500ms

Type "q"
Sleep 600ms
