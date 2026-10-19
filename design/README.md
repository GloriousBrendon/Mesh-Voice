# Mesh design handoff

    design/
      DESIGN.md           spec: tokens, screens, code mapping, bridge rules
      HANDOFF-PROMPT.md   prompt for Claude Code
      theme.toml          new theme format with rice defaults
      screens/            the four mockups (.dc.html) + canvas.json layout

Use:

    cd ~/path/to/Mesh-Voice
    unzip ~/Downloads/mesh-design.zip      # creates ./design
    claude "$(cat design/HANDOFF-PROMPT.md)"
