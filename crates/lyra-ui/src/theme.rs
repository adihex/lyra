//! Lyra's "bubble" theme approximated in GTK CSS — palette mirrors
//! LyraTheme.swift's dark row (chassis/legend/tint/mint).

pub const CSS: &str = r#"
window { background: #241F2E; color: #ECE5F2; }
.lyra-sidebar { background: #1D1926; }
.lyra-sidebar row { padding: 8px 12px; border-radius: 10px; font-weight: 500; }
.lyra-sidebar row:selected { background: #C5B3D8; color: #30263F; }
.lyra-now-playing { background: #1D1926; border-top: 1px solid #3A3150;
                    padding: 10px 16px; }
.lyra-card { background: #2C2638; border: 1px solid #3A3150;
             border-radius: 12px; padding: 12px; }
button.sharp { border-radius: 8px; padding: 5px 12px; }
button.suggested-action { background: #C5B3D8; color: #30263F;
                          border-radius: 8px; padding: 5px 14px; }
button.suggested-action:disabled { opacity: 0.45; }
button.flat { border-radius: 8px; }
.mint { color: #A9C9C3; }
.dim { color: #8E82A8; }
.track-header button { font-weight: 600; font-size: 11px; color: #8E82A8;
                       background: none; border: none; padding: 6px 4px; }
.track-header button:hover { color: #ECE5F2; }
row.track { padding: 4px 8px; }
row.track:selected { background: rgba(197,179,216,0.18); }
scale.seek trough { min-height: 4px; }
scale.seek highlight { background: #C5B3D8; }
scale.eq trough { min-width: 4px; }
scale.eq highlight { background: #A9C9C3; }
entry, spinbutton { background: #241F2E; border: 1px solid #3A3150;
                    border-radius: 8px; padding: 5px 10px; }
"#;
