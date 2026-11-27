param(
    [string]$ReferenceRoot = (Resolve-Path (Join-Path $PSScriptRoot "../../../../UxPlay")).Path
)

$header = Get-Content (Join-Path $ReferenceRoot "lib/playfair/omg_hax.h") -Raw
$source = Get-Content (Join-Path $ReferenceRoot "lib/playfair/omg_hax.c") -Raw
$adapter = Get-Content (Join-Path $ReferenceRoot "lib/fairplay_playfair.c") -Raw
$output = Join-Path $PSScriptRoot "../src/tables.rs"
$binaryOutput = Join-Path $PSScriptRoot "../tables.bin"
$arrays = @(
    @{ Name = "MESSAGE_KEY"; Source = $header; CName = "message_key"; RustType = "u8" },
    @{ Name = "MESSAGE_IV"; Source = $header; CName = "message_iv"; RustType = "u8" },
    @{ Name = "Z_KEY"; Source = $header; CName = "z_key"; RustType = "u8" },
    @{ Name = "X_KEY"; Source = $header; CName = "x_key"; RustType = "u8" },
    @{ Name = "T_KEY"; Source = $header; CName = "t_key"; RustType = "u8" },
    @{ Name = "TABLE_S1"; Source = $header; CName = "table_s1"; RustType = "u8" },
    @{ Name = "TABLE_S2"; Source = $header; CName = "table_s2"; RustType = "u8" },
    @{ Name = "TABLE_S3"; Source = $header; CName = "table_s3"; RustType = "u8" },
    @{ Name = "TABLE_S4"; Source = $header; CName = "table_s4"; RustType = "u8" },
    @{ Name = "TABLE_S5"; Source = $header; CName = "table_s5"; RustType = "u32" },
    @{ Name = "TABLE_S6"; Source = $header; CName = "table_s6"; RustType = "u32" },
    @{ Name = "TABLE_S7"; Source = $header; CName = "table_s7"; RustType = "u32" },
    @{ Name = "TABLE_S8"; Source = $header; CName = "table_s8"; RustType = "u32" },
    @{ Name = "TABLE_S9"; Source = $header; CName = "table_s9"; RustType = "u32" },
    @{ Name = "TABLE_S10"; Source = $header; CName = "table_s10"; RustType = "u8" },
    @{ Name = "INITIAL_SESSION_KEY"; Source = $source; CName = "initial_session_key"; RustType = "u8" },
    @{ Name = "STATIC_SOURCE_1"; Source = $source; CName = "static_source_1"; RustType = "u8" },
    @{ Name = "STATIC_SOURCE_2"; Source = $source; CName = "static_source_2"; RustType = "u8" },
    @{ Name = "DEFAULT_SAP"; Source = $source; CName = "default_sap"; RustType = "u8" },
    @{ Name = "FP_HEADER"; Source = $adapter; CName = "fp_header"; RustType = "u8" },
    @{ Name = "LEGACY_REPLY_MESSAGE"; Source = $adapter; CName = "reply_message"; RustType = "u8" }
)

$binary = [System.Collections.Generic.List[byte]]::new()
$metadata = [System.Collections.Generic.List[object]]::new()
foreach ($array in $arrays) {
    $pattern = "(?s)\b(?:unsigned char|unsigned long|uint32_t)\s+$($array.CName)\s*(?:\[[^=;]*\])+\s*=\s*\{(.*?)\};"
    if ($array.CName -in @("initial_session_key", "static_source_1", "static_source_2", "default_sap")) {
        $pattern = "(?s)\bunsigned char\s+$($array.CName)\s*\[\]\s*=\s*\{(.*?)\};"
    }
    if ($array.CName -eq "fp_header") {
        $pattern = "(?s)\bstatic const unsigned char\s+fp_header\s*\[\]\s*=\s*\{(.*?)\};"
    }
    if ($array.CName -eq "reply_message") {
        $pattern = "(?s)\bstatic const unsigned char\s+reply_message\s*\[4\]\[142\]\s*=\s*\{(.*?)\};"
    }
    $match = [regex]::Match($array.Source, $pattern)
    if (!$match.Success) { throw "Could not locate source array $($array.CName)" }
    $values = [regex]::Matches($match.Groups[1].Value, "0[xX][0-9a-fA-F]+|\b\d+\b") | ForEach-Object {
        $literal = $_.Value
        if ($literal.StartsWith("0x", [System.StringComparison]::OrdinalIgnoreCase)) {
            [Convert]::ToUInt64($literal.Substring(2), 16)
        } else {
            [UInt64]$literal
        }
    }
    if ($values.Count -eq 0) { throw "Empty source array $($array.CName)" }
    $offset = $binary.Count
    foreach ($value in $values) {
        if ($array.RustType -eq "u8") {
            $binary.Add([byte]$value)
        } else {
            $word = [uint32]$value
            $binary.Add([byte]($word -band 0xff))
            $binary.Add([byte](($word -shr 8) -band 0xff))
            $binary.Add([byte](($word -shr 16) -band 0xff))
            $binary.Add([byte](($word -shr 24) -band 0xff))
        }
    }
    $metadata.Add(@{ Name = $array.Name; RustType = $array.RustType; Count = $values.Count; Offset = $offset })
}

[System.IO.File]::WriteAllBytes($binaryOutput, $binary.ToArray())
$builder = [System.Text.StringBuilder]::new()
[void]$builder.AppendLine("//! Protocol tables and fixed data ported from UxPlay's PlayFair implementation.")
[void]$builder.AppendLine("//! Source tables: `lib/playfair/omg_hax.h`, `lib/playfair/omg_hax.c`, and `lib/fairplay_playfair.c`.")
[void]$builder.AppendLine("//! Copyright notices are in `LICENSE-NOTICE.md`; GPL-3.0 text is in `licenses/PLAYFAIR-GPL-3.0.md`.")
[void]$builder.AppendLine('const DATA: &[u8] = include_bytes!("../tables.bin");')
[void]$builder.AppendLine()
[void]$builder.AppendLine("const fn byte_array<const N: usize>(offset: usize) -> [u8; N] {")
[void]$builder.AppendLine("    let mut output = [0; N];")
[void]$builder.AppendLine("    let mut index = 0;")
[void]$builder.AppendLine("    while index < N {")
[void]$builder.AppendLine("        output[index] = DATA[offset + index];")
[void]$builder.AppendLine("        index += 1;")
[void]$builder.AppendLine("    }")
[void]$builder.AppendLine("    output")
[void]$builder.AppendLine("}")
[void]$builder.AppendLine()
[void]$builder.AppendLine("const fn word_array<const N: usize>(offset: usize) -> [u32; N] {")
[void]$builder.AppendLine("    let mut output = [0; N];")
[void]$builder.AppendLine("    let mut index = 0;")
[void]$builder.AppendLine("    while index < N {")
[void]$builder.AppendLine("        let byte = offset + index * 4;")
[void]$builder.AppendLine("        output[index] =")
[void]$builder.AppendLine("            u32::from_le_bytes([DATA[byte], DATA[byte + 1], DATA[byte + 2], DATA[byte + 3]]);")
[void]$builder.AppendLine("        index += 1;")
[void]$builder.AppendLine("    }")
[void]$builder.AppendLine("    output")
[void]$builder.AppendLine("}")
[void]$builder.AppendLine()
foreach ($item in $metadata) {
    $reader = if ($item.RustType -eq "u8") { "byte_array" } else { "word_array" }
    [void]$builder.AppendLine("pub static $($item.Name): [$($item.RustType); $($item.Count)] = $reader::<$($item.Count)>($($item.Offset));")
}
[System.IO.File]::WriteAllText($output, $builder.ToString(), [System.Text.UTF8Encoding]::new($false))
Write-Output "Wrote $output and $binaryOutput ($($binary.Count) bytes)"
