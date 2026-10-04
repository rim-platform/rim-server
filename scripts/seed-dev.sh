#!/usr/bin/env bash
set -euo pipefail

###############################################################################
# RIM Development Seed Script
#
# Bootstraps a fresh RIM instance with test data via the GraphQL API.
# Requires: curl, jq, a running RIM server.
#
# Environment variables:
#   RIM_API_URL          - GraphQL endpoint (default: http://localhost:8080/api)
#   RIM_BOOTSTRAP_SECRET - Bootstrap secret configured on the server
#
# Usage:
#   RIM_BOOTSTRAP_SECRET=mysecret ./scripts/seed-dev.sh
###############################################################################

API_URL="${RIM_API_URL:-http://localhost:8080/api}"
BOOTSTRAP_SECRET="${RIM_BOOTSTRAP_SECRET:-}"

ADMIN_USER="admin"
ADMIN_PASS="admin123"
TEST_USER="testuser"
TEST_PASS="test123"
SERVER_NAME="Dev Workspace"

ADMIN_COOKIE_JAR=$(mktemp)
TEST_COOKIE_JAR=$(mktemp)
trap 'rm -f "$ADMIN_COOKIE_JAR" "$TEST_COOKIE_JAR"' EXIT

echo "=== RIM Development Seed Script ==="
echo "API: $API_URL"
echo ""

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

gql() {
    # gql <cookie_jar> <operation_name> <query> [variables_json]
    local jar="$1" op="$2" query="$3" vars="${4:-{}}"
    local csrf=""

    # Read CSRF token from cookie jar if present (rim_csrf cookie)
    if [[ -f "$jar" ]]; then
        csrf=$(grep 'rim_csrf' "$jar" 2>/dev/null | awk '{print $NF}' || true)
    fi

    local -a headers=(-H "Content-Type: application/json")
    if [[ -n "$csrf" ]]; then
        headers+=(-H "X-CSRF-Token: $csrf")
    fi

    local payload
    payload=$(jq -n \
        --arg q "$query" \
        --arg op "$op" \
        --argjson v "$vars" \
        '{query: $q, operationName: $op, variables: $v}')

    curl -s -S \
        -b "$jar" \
        -c "$jar" \
        "${headers[@]}" \
        -d "$payload" \
        "$API_URL"
}

check_errors() {
    # check_errors <response_json> <step_description>
    local resp="$1" step="$2"
    local errors
    errors=$(echo "$resp" | jq -r '.errors // empty')
    if [[ -n "$errors" && "$errors" != "null" ]]; then
        echo "FAILED: $step"
        echo "$resp" | jq '.errors'
        exit 1
    fi
}

# ---------------------------------------------------------------------------
# Pre-flight checks
# ---------------------------------------------------------------------------

if ! command -v jq &>/dev/null; then
    echo "Error: jq is required but not installed."
    exit 1
fi

if ! command -v curl &>/dev/null; then
    echo "Error: curl is required but not installed."
    exit 1
fi

if [[ -z "$BOOTSTRAP_SECRET" ]]; then
    echo "Error: RIM_BOOTSTRAP_SECRET is not set."
    echo "Set it to match the server's RIM_BOOTSTRAP_SECRET value."
    exit 1
fi

# ---------------------------------------------------------------------------
# 1. Bootstrap: create admin user + workspace
# ---------------------------------------------------------------------------
echo "--- Step 1: Bootstrap (create admin + workspace) ---"

BOOTSTRAP_QUERY='mutation bootstrap($input: BootstrapInput!) {
  bootstrap(input: $input) {
    userId
    serverId
  }
}'

BOOTSTRAP_VARS=$(jq -n \
    --arg secret "$BOOTSTRAP_SECRET" \
    --arg username "$ADMIN_USER" \
    --arg password "$ADMIN_PASS" \
    --arg serverName "$SERVER_NAME" \
    '{input: {secret: $secret, username: $username, password: $password, serverName: $serverName}}')

BOOTSTRAP_RESP=$(gql "$ADMIN_COOKIE_JAR" "bootstrap" "$BOOTSTRAP_QUERY" "$BOOTSTRAP_VARS")
check_errors "$BOOTSTRAP_RESP" "bootstrap"

ADMIN_USER_ID=$(echo "$BOOTSTRAP_RESP" | jq -r '.data.bootstrap.userId')
SERVER_ID=$(echo "$BOOTSTRAP_RESP" | jq -r '.data.bootstrap.serverId')

echo "  Admin user ID : $ADMIN_USER_ID"
echo "  Server ID     : $SERVER_ID"
echo ""

# ---------------------------------------------------------------------------
# 2. Create invite for the workspace
# ---------------------------------------------------------------------------
echo "--- Step 2: Create invite ---"

CREATE_INVITE_QUERY='mutation createInvite($input: CreateInviteInput!) {
  createInvite(input: $input) {
    inviteToken
    inviteId
  }
}'

CREATE_INVITE_VARS=$(jq -n \
    --arg serverId "$SERVER_ID" \
    '{input: {serverId: $serverId, maxUses: 1}}')

INVITE_RESP=$(gql "$ADMIN_COOKIE_JAR" "createInvite" "$CREATE_INVITE_QUERY" "$CREATE_INVITE_VARS")
check_errors "$INVITE_RESP" "createInvite"

INVITE_TOKEN=$(echo "$INVITE_RESP" | jq -r '.data.createInvite.inviteToken')
INVITE_ID=$(echo "$INVITE_RESP" | jq -r '.data.createInvite.inviteId')

echo "  Invite ID    : $INVITE_ID"
echo "  Invite token : $INVITE_TOKEN"
echo ""

# ---------------------------------------------------------------------------
# 3. Accept invite: create second test user
# ---------------------------------------------------------------------------
echo "--- Step 3: Accept invite (create test user) ---"

ACCEPT_INVITE_QUERY='mutation accept_invite($input: AcceptInviteInput!) {
  acceptInvite(input: $input) {
    userId
    serverId
  }
}'

ACCEPT_INVITE_VARS=$(jq -n \
    --arg inviteToken "$INVITE_TOKEN" \
    --arg username "$TEST_USER" \
    --arg password "$TEST_PASS" \
    '{input: {inviteToken: $inviteToken, username: $username, password: $password}}')

ACCEPT_RESP=$(gql "$TEST_COOKIE_JAR" "accept_invite" "$ACCEPT_INVITE_QUERY" "$ACCEPT_INVITE_VARS")
check_errors "$ACCEPT_RESP" "acceptInvite"

TEST_USER_ID=$(echo "$ACCEPT_RESP" | jq -r '.data.acceptInvite.userId')

echo "  Test user ID : $TEST_USER_ID"
echo ""

# ---------------------------------------------------------------------------
# 4. Create channels (as admin)
# ---------------------------------------------------------------------------
echo "--- Step 4: Create channels ---"

CREATE_CHANNEL_QUERY='mutation createConversation($input: CreateConversationInput!) {
  createConversation(input: $input) {
    id
    title
  }
}'

declare -a CHANNEL_IDS=()
declare -a CHANNEL_NAMES=("general" "random" "dev")

for ch_name in "${CHANNEL_NAMES[@]}"; do
    CH_VARS=$(jq -n \
        --arg serverId "$SERVER_ID" \
        --arg title "$ch_name" \
        '{input: {serverId: $serverId, isChannel: true, title: $title}}')

    CH_RESP=$(gql "$ADMIN_COOKIE_JAR" "createConversation" "$CREATE_CHANNEL_QUERY" "$CH_VARS")
    check_errors "$CH_RESP" "createConversation ($ch_name)"

    CH_ID=$(echo "$CH_RESP" | jq -r '.data.createConversation.id')
    CHANNEL_IDS+=("$CH_ID")
    echo "  #$ch_name : $CH_ID"
done
echo ""

# ---------------------------------------------------------------------------
# 5. Send test messages
# ---------------------------------------------------------------------------
echo "--- Step 5: Send test messages ---"

SEND_MSG_QUERY='mutation sendMessage($input: SendMessageInput!) {
  sendMessage(input: $input) {
    id
    text
  }
}'

send_msg() {
    # send_msg <cookie_jar> <conversation_id> <text>
    local jar="$1" conv_id="$2" text="$3"
    local vars
    vars=$(jq -n \
        --arg serverId "$SERVER_ID" \
        --arg conversationId "$conv_id" \
        --arg text "$text" \
        '{input: {serverId: $serverId, conversationId: $conversationId, text: $text}}')

    local resp
    resp=$(gql "$jar" "sendMessage" "$SEND_MSG_QUERY" "$vars")
    check_errors "$resp" "sendMessage"

    local msg_id
    msg_id=$(echo "$resp" | jq -r '.data.sendMessage.id')
    echo "  [$msg_id] $text"
}

# Messages in #general (index 0)
GENERAL_ID="${CHANNEL_IDS[0]}"
send_msg "$ADMIN_COOKIE_JAR" "$GENERAL_ID" "Welcome to the dev workspace!"
send_msg "$TEST_COOKIE_JAR"  "$GENERAL_ID" "Thanks! Excited to be here."
send_msg "$ADMIN_COOKIE_JAR" "$GENERAL_ID" "Feel free to explore the channels."

# Messages in #random (index 1)
RANDOM_ID="${CHANNEL_IDS[1]}"
send_msg "$ADMIN_COOKIE_JAR" "$RANDOM_ID" "This is the random channel for off-topic chat."
send_msg "$TEST_COOKIE_JAR"  "$RANDOM_ID" "Nice, good to have a place for that."

# Messages in #dev (index 2)
DEV_ID="${CHANNEL_IDS[2]}"
send_msg "$ADMIN_COOKIE_JAR" "$DEV_ID" "Post your dev questions here."
send_msg "$TEST_COOKIE_JAR"  "$DEV_ID" "How do I run the test suite?"
send_msg "$ADMIN_COOKIE_JAR" "$DEV_ID" "cargo test --workspace"

echo ""

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------
echo "==========================================="
echo "  Seed complete!"
echo "==========================================="
echo ""
echo "  Server : $SERVER_NAME (ID: $SERVER_ID)"
echo ""
echo "  Admin user:"
echo "    Username : $ADMIN_USER"
echo "    Password : $ADMIN_PASS"
echo "    User ID  : $ADMIN_USER_ID"
echo ""
echo "  Test user:"
echo "    Username : $TEST_USER"
echo "    Password : $TEST_PASS"
echo "    User ID  : $TEST_USER_ID"
echo ""
echo "  Channels:"
for i in "${!CHANNEL_NAMES[@]}"; do
    echo "    #${CHANNEL_NAMES[$i]} : ${CHANNEL_IDS[$i]}"
done
echo ""
echo "  To log in as admin:"
echo "    curl -c cookies.txt $API_URL \\"
echo "      -H 'Content-Type: application/json' \\"
echo "      -d '{\"query\":\"mutation login(\$input: LoginInput!) { login(input: \$input) { userId } }\",\"operationName\":\"login\",\"variables\":{\"input\":{\"username\":\"$ADMIN_USER\",\"password\":\"$ADMIN_PASS\"}}}'"
echo ""
