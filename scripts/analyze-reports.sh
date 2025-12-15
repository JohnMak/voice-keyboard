#!/bin/bash
# Analyze voice-keyboard reports for transcription quality
# Usage: ./analyze-reports.sh [--new-only]

REPORTS_DIR="${REPORTS_DIR:-/home/alexmak/voice-keyboard/materials/reports}"
ANALYSIS_LOG="$REPORTS_DIR/analysis_log.md"
ANALYZED_MARKER="$REPORTS_DIR/.analyzed"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

new_only=false
if [[ "$1" == "--new-only" ]]; then
    new_only=true
fi

echo "=== Voice Keyboard Report Analysis ==="
echo "Reports directory: $REPORTS_DIR"
echo ""

# Find all report directories
reports=$(find "$REPORTS_DIR" -maxdepth 1 -type d -name "20*" | sort)

if [[ -z "$reports" ]]; then
    echo "No reports found."
    exit 0
fi

# Load already analyzed reports
declare -A analyzed
if [[ -f "$ANALYZED_MARKER" ]]; then
    while IFS= read -r line; do
        analyzed["$line"]=1
    done < "$ANALYZED_MARKER"
fi

# Summary stats
total_reports=0
new_reports=0
total_fragments=0
matching_fragments=0

for report_dir in $reports; do
    report_name=$(basename "$report_dir")
    total_reports=$((total_reports + 1))

    # Skip if already analyzed and --new-only
    if [[ "$new_only" == true ]] && [[ "${analyzed[$report_name]}" == "1" ]]; then
        continue
    fi

    new_reports=$((new_reports + 1))

    echo -e "${YELLOW}=== Report: $report_name ===${NC}"

    # Check for report.json
    if [[ -f "$report_dir/report.json" ]]; then
        # Parse report
        full_text=$(jq -r '.full_transcription // "N/A"' "$report_dir/report.json" 2>/dev/null)
        fragments=$(jq -r '.fragments[]? | "\(.index): \(.transcription)"' "$report_dir/report.json" 2>/dev/null)
        combined=$(jq -r '.fragments[]?.transcription' "$report_dir/report.json" 2>/dev/null | tr '\n' ' ')

        echo ""
        echo -e "${GREEN}Full transcription:${NC}"
        echo "$full_text"
        echo ""
        echo -e "${GREEN}Fragments:${NC}"
        echo "$fragments"
        echo ""
        echo -e "${GREEN}Combined fragments:${NC}"
        echo "$combined"
        echo ""

        # Compare
        if [[ "$full_text" != "N/A" ]] && [[ -n "$combined" ]]; then
            # Simple word count comparison
            full_words=$(echo "$full_text" | wc -w | tr -d ' ')
            combined_words=$(echo "$combined" | wc -w | tr -d ' ')

            echo -e "${GREEN}Word count: Full=$full_words, Combined=$combined_words${NC}"

            if [[ "$full_words" -eq "$combined_words" ]]; then
                echo -e "${GREEN}Word count matches!${NC}"
            else
                diff=$((full_words - combined_words))
                if [[ $diff -gt 0 ]]; then
                    echo -e "${RED}Missing $diff words in fragments${NC}"
                else
                    echo -e "${YELLOW}Extra $((-diff)) words in fragments${NC}"
                fi
            fi
        fi
    else
        echo -e "${RED}No report.json found${NC}"

        # Try to read individual files
        if [[ -f "$report_dir/full_transcription.txt" ]]; then
            echo -e "${GREEN}Full transcription:${NC}"
            cat "$report_dir/full_transcription.txt"
        fi
    fi

    echo ""
    echo "---"
    echo ""

    # Mark as analyzed
    echo "$report_name" >> "$ANALYZED_MARKER"
done

echo ""
echo "=== Summary ==="
echo "Total reports: $total_reports"
echo "Analyzed this run: $new_reports"

# Update analysis log
{
    echo ""
    echo "## Analysis $(date '+%Y-%m-%d %H:%M:%S')"
    echo ""
    echo "- Reports analyzed: $new_reports"
    echo "- Total reports: $total_reports"
} >> "$ANALYSIS_LOG"
