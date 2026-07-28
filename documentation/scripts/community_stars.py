#!/usr/bin/env python3
"""
Community Stars Analysis Script for PondSec/ponduin repository

This script analyzes GitHub contributor statistics and generates rankings for:
- Top 5 Community All-Stars (External contributors)
- Top 5 Team Stars (PondSec employees, non-ponduin team)
- Monthly Leaderboard (all eligible contributors)

The script automatically:
- Fetches contributor data from GitHub API (with retry logic)
- Checks public org memberships to detect PondSec employees
- Categorizes contributors as PondSec or External
- Caches data locally for faster subsequent runs

Usage:
    python3 community_stars.py "November 2025"
    python3 community_stars.py "November 1, 2025 - November 17, 2025"
    python3 community_stars.py "2025-11-01 - 2025-11-17"

Requirements:
    - Internet connection (to fetch GitHub data)
    - Team list file at documentation/scripts/community_stars_teams.txt
"""

import calendar
import json
import re
import sys
import time
import urllib.request
from datetime import datetime
from pathlib import Path

# GitHub URL for team list file
TEAMS_FILE_URL = "https://raw.githubusercontent.com/PondSec/ponduin/main/documentation/scripts/community_stars_teams.txt"
LOCAL_TEAMS_FILE = Path(__file__).parent / "community_stars_teams.txt"

# PondSec-related organizations to check
PONDSEC_ORGS = {"pondsec"}


def is_pondsec_member(username):
    """Check if a user is a PondSec employee by checking their profile and org memberships.

    Makes a single API call to get user profile (includes company field),
    then only calls orgs endpoint if company field doesn't match.
    """
    try:
        # First check the user's profile (single API call)
        url = f"https://api.github.com/users/{username}"
        with urllib.request.urlopen(url) as response:
            user_data = json.loads(response.read().decode("utf-8"))

        # Check company field first (no additional API call needed)
        company = (
            user_data.get("company", "").lower() if user_data.get("company") else ""
        )
        if company:
            # Check for PondSec-related keywords in company field
            pondsec_keywords = ["pondsec", "ponduin"]
            if any(keyword in company for keyword in pondsec_keywords):
                return True

        # Only check orgs if company field didn't match (second API call only when needed)
        url = f"https://api.github.com/users/{username}/orgs"
        with urllib.request.urlopen(url) as response:
            orgs = json.loads(response.read().decode("utf-8"))

        # Check if any org matches PondSec orgs (case-insensitive)
        user_orgs = {org["login"].lower() for org in orgs}
        if user_orgs & PONDSEC_ORGS:
            return True

        return False

    except Exception:
        # If we can't check (rate limit, network error, etc.), return False
        # This means we'll default to treating them as external
        return False


def load_team_lists():
    """Load and parse team lists from file (local or GitHub)."""
    content = None

    # Try local file first
    if LOCAL_TEAMS_FILE.exists():
        with open(LOCAL_TEAMS_FILE, "r") as f:
            content = f.read()
    else:
        # Fall back to GitHub
        try:
            with urllib.request.urlopen(TEAMS_FILE_URL) as response:
                content = response.read().decode("utf-8")
        except Exception as e:
            print(f"Error: Could not load team list file from {TEAMS_FILE_URL}")
            print(f"Details: {e}")
            sys.exit(1)

    # Parse the team lists
    ponduin_maintainers = set()
    pondsec_non_ponduin = set()
    external_ponduin = set()
    bots = set()

    current_section = None
    for line in content.split("\n"):
        line = line.strip()

        # Skip comments and empty lines
        if not line or line.startswith("#"):
            # Check for section headers in comments
            if "# Ponduin Maintainers" in line:
                current_section = "ponduin_maintainers"
            elif "# PondSec, non-ponduin" in line:
                current_section = "pondsec_non_ponduin"
            elif "# External, ponduin" in line:
                current_section = "external_ponduin"
            elif "# Bots" in line:
                current_section = "bots"
            continue

        # Add username to appropriate set (lowercase for case-insensitive matching)
        username = line.lower()
        if current_section == "ponduin_maintainers":
            ponduin_maintainers.add(username)
        elif current_section == "pondsec_non_ponduin":
            pondsec_non_ponduin.add(username)
        elif current_section == "external_ponduin":
            external_ponduin.add(username)
        elif current_section == "bots":
            bots.add(username)

    return ponduin_maintainers, pondsec_non_ponduin, external_ponduin, bots


def parse_date_range(date_input):
    """Parse various date input formats and return start/end timestamps."""
    date_input = date_input.strip()

    # Format: "Month YYYY" (e.g., "November 2025")
    month_year_pattern = r"^(January|February|March|April|May|June|July|August|September|October|November|December)\s+(\d{4})$"
    match = re.match(month_year_pattern, date_input, re.IGNORECASE)
    if match:
        month_name = match.group(1).capitalize()
        year = int(match.group(2))
        start_date = datetime.strptime(f"{month_name} 1 {year}", "%B %d %Y")
        last_day = calendar.monthrange(year, start_date.month)[1]
        end_date = datetime(year, start_date.month, last_day, 23, 59, 59)
        return start_date.timestamp(), end_date.timestamp(), date_input

    # Format: "Date1 - Date2" (e.g., "November 1, 2025 - November 17, 2025" or "2025-11-01 - 2025-11-17")
    if " - " in date_input or " to " in date_input:
        separator = " - " if " - " in date_input else " to "
        parts = date_input.split(separator)
        if len(parts) == 2:
            date_formats = ["%B %d, %Y", "%b %d, %Y", "%Y-%m-%d"]
            start_date = None
            end_date = None

            for fmt in date_formats:
                try:
                    start_date = datetime.strptime(parts[0].strip(), fmt)
                    end_date = datetime.strptime(parts[1].strip(), fmt)
                    break
                except ValueError:
                    continue

            if start_date and end_date:
                end_date = datetime(
                    end_date.year, end_date.month, end_date.day, 23, 59, 59
                )
                return start_date.timestamp(), end_date.timestamp(), date_input

    raise ValueError(
        f"Could not parse date input: {date_input}\nSupported formats:\n  - 'Month YYYY' (e.g., 'November 2025')\n  - 'Month Day, YYYY - Month Day, YYYY' (e.g., 'November 1, 2025 - November 17, 2025')\n  - 'YYYY-MM-DD - YYYY-MM-DD' (e.g., '2025-11-01 - 2025-11-17')"
    )


def main():
    # Parse command line arguments
    if len(sys.argv) < 2:
        print("Usage: python3 community_stars.py 'date_range'")
        print("Examples:")
        print("  python3 community_stars.py 'November 2025'")
        print("  python3 community_stars.py 'November 1, 2025 - November 17, 2025'")
        print("  python3 community_stars.py '2025-11-01 - 2025-11-17'")
        sys.exit(1)

    date_input = sys.argv[1]
    try:
        start_timestamp, end_timestamp, display_period = parse_date_range(date_input)
        start_date = datetime.fromtimestamp(start_timestamp)
        end_date = datetime.fromtimestamp(end_timestamp)
    except ValueError as e:
        print(f"Error: {e}")
        sys.exit(1)

    # Load team lists
    ponduin_maintainers, pondsec_non_ponduin, external_ponduin, bots = load_team_lists()

    # Load GitHub data
    github_data_file = "/tmp/github_contributors.json"
    contributors_data = None

    # Try to load existing file first
    try:
        with open(github_data_file, "r") as f:
            contributors_data = json.load(f)

        # Validate the data is not empty or invalid
        if (
            not contributors_data
            or not isinstance(contributors_data, list)
            or len(contributors_data) == 0
        ):
            print(
                f"Warning: GitHub data file exists but is empty or invalid. Fetching fresh data...",
                file=sys.stderr,
            )
            contributors_data = None
    except (FileNotFoundError, json.JSONDecodeError):
        print(
            f"GitHub data file not found or invalid. Fetching fresh data...",
            file=sys.stderr,
        )
        contributors_data = None

    # Fetch from GitHub API if needed
    if contributors_data is None:
        print("Fetching contributor data from GitHub API...", file=sys.stderr)
        max_retries = 3
        retry_delay = 2

        for attempt in range(max_retries):
            try:
                url = "https://api.github.com/repos/PondSec/ponduin/stats/contributors"
                with urllib.request.urlopen(url, timeout=30) as response:
                    contributors_data = json.loads(response.read().decode("utf-8"))

                # Validate the response
                if (
                    contributors_data
                    and isinstance(contributors_data, list)
                    and len(contributors_data) > 0
                ):
                    # Save to file for future use
                    with open(github_data_file, "w") as f:
                        json.dump(contributors_data, f)
                    print(
                        f"✓ Successfully fetched data for {len(contributors_data)} contributors",
                        file=sys.stderr,
                    )
                    break
                else:
                    print(
                        f"Attempt {attempt + 1}/{max_retries}: GitHub API returned empty data. Retrying...",
                        file=sys.stderr,
                    )
                    contributors_data = None
                    if attempt < max_retries - 1:
                        time.sleep(retry_delay)
            except Exception as e:
                print(
                    f"Attempt {attempt + 1}/{max_retries}: Error fetching from GitHub API: {e}",
                    file=sys.stderr,
                )
                if attempt < max_retries - 1:
                    time.sleep(retry_delay)
                else:
                    print(
                        "\nError: Could not fetch GitHub contributor data after multiple attempts."
                    )
                    print(
                        "The GitHub stats API may be temporarily unavailable or still computing statistics."
                    )
                    print("Please try again in a few minutes.")
                    sys.exit(1)

        if contributors_data is None:
            print("\nError: GitHub API returned empty data after multiple attempts.")
            print(
                "The repository statistics may still be computing. Please try again in a few minutes."
            )
            sys.exit(1)

    # Process contributors
    contributor_stats = []
    checked_orgs = {}  # Cache org checks to avoid redundant API calls

    print("Checking contributor organizations...", file=sys.stderr)

    for contributor in contributors_data:
        # Skip if author is None (deleted users)
        if contributor.get("author") is None:
            continue

        username = contributor["author"]["login"]
        username_lower = username.lower()

        # Skip excluded categories (case-insensitive matching)
        if (
            username_lower in bots
            or username_lower in ponduin_maintainers
            or username_lower in external_ponduin
        ):
            continue

        # Calculate stats for the specified period
        period_commits = 0
        period_additions = 0
        period_deletions = 0

        for week in contributor["weeks"]:
            week_timestamp = week["w"]
            if start_timestamp <= week_timestamp <= end_timestamp:
                period_commits += week["c"]
                period_additions += week["a"]
                period_deletions += week["d"]

        # Only include contributors with activity in the period
        if period_commits > 0:
            total_lines = period_additions + period_deletions

            # Categorize (only PondSec non-ponduin and External now)
            if username_lower in pondsec_non_ponduin:
                category = "pondsec_non_ponduin"
            else:
                # Check if user is in a PondSec org (with caching)
                if username not in checked_orgs:
                    checked_orgs[username] = is_pondsec_member(username)
                    # Add a small delay to avoid rate limiting
                    time.sleep(0.1)

                if checked_orgs[username]:
                    category = "pondsec_non_ponduin"
                    print(f"  ✓ Detected PondSec employee: @{username}", file=sys.stderr)
                else:
                    category = "external"

            contributor_stats.append(
                {
                    "username": username,
                    "category": category,
                    "commits": period_commits,
                    "additions": period_additions,
                    "deletions": period_deletions,
                    "total_lines": total_lines,
                    "score": period_commits + total_lines,
                }
            )

    # Sort by score
    contributor_stats.sort(key=lambda x: x["score"], reverse=True)

    # Separate by category
    pondsec_list = [c for c in contributor_stats if c["category"] == "pondsec_non_ponduin"]
    external_list = [c for c in contributor_stats if c["category"] == "external"]

    # Get top 5 from each
    top_external = external_list[:5]
    top_internal = pondsec_list[:5]

    # Print results
    print("=" * 70)
    print(f"COMMUNITY STARS - {display_period.upper()}")
    print(
        f"(Period: {start_date.strftime('%B %d, %Y')} - {end_date.strftime('%B %d, %Y')})"
    )
    print("=" * 70)
    print()

    print("🏆 TOP 5 COMMUNITY ALL-STARS (External Contributors)")
    print("-" * 70)
    if top_external:
        for i, contrib in enumerate(top_external, 1):
            print(
                f"{i}. @{contrib['username']:20s} - {contrib['commits']:3d} commits, {contrib['total_lines']:6,d} lines"
            )
    else:
        print("No external contributors found for this period.")

    print()
    print("⭐ TOP 5 TEAM STARS (PondSec, non-ponduin)")
    print("-" * 70)
    if top_internal:
        for i, contrib in enumerate(top_internal, 1):
            print(
                f"{i}. @{contrib['username']:20s} - {contrib['commits']:3d} commits, {contrib['total_lines']:6,d} lines"
            )
    else:
        print("No internal contributors found for this period.")

    print()
    print("📊 MONTHLY LEADERBOARD (All Contributors)")
    print("-" * 70)
    if contributor_stats:
        for i, contrib in enumerate(contributor_stats, 1):
            cat_label = "External" if contrib["category"] == "external" else "PondSec"
            print(
                f"{i:2d}. @{contrib['username']:20s} - {contrib['commits']:3d} commits, {contrib['total_lines']:6,d} lines [{cat_label}]"
            )
    else:
        print("No contributors found for this period.")

    print()
    print("=" * 70)
    print(
        f"Total contributors (excluding bots, ponduin maintainers, external ponduin): {len(contributor_stats)}"
    )
    print(f"  External: {len(external_list)}")
    print(f"  PondSec (non-ponduin): {len(pondsec_list)}")
    print("=" * 70)


if __name__ == "__main__":
    main()
