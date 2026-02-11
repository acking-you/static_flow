#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

step() {
    echo
    echo "==> $*"
}

fail() {
    echo "[FAIL] $*" >&2
    exit 1
}

assert_eq() {
    local expected="$1"
    local actual="$2"
    local message="$3"
    if [[ "$expected" != "$actual" ]]; then
        fail "$message (expected=$expected actual=$actual)"
    fi
}

PROFILE="${BUILD_PROFILE:-debug}"
if [[ -z "${CLI_BIN:-}" ]]; then
    if [[ "$PROFILE" == "release" ]]; then
        step "Building sf-cli (release)"
        cargo build --release -p sf-cli
        CLI_BIN="./target/release/sf-cli"
    else
        step "Building sf-cli (debug)"
        cargo build -p sf-cli
        CLI_BIN="./target/debug/sf-cli"
    fi
fi

[[ -x "$CLI_BIN" ]] || fail "CLI binary not executable: $CLI_BIN"

echo "Using CLI binary: $CLI_BIN"

WORKDIR="${WORKDIR:-./tmp/cli-e2e-run}"
DB_PATH="$WORKDIR/lancedb"
OUT_DIR="$WORKDIR/out"
NOTES_DIR="$WORKDIR/notes"
META_FILE="$WORKDIR/doc_meta.tsv"

rm -rf "$WORKDIR"
mkdir -p "$WORKDIR" "$OUT_DIR" "$NOTES_DIR"

extract_title() {
    local file="$1"
    awk '/^# /{sub(/^# /, ""); print; exit}' "$file"
}

extract_summary() {
    local file="$1"
    awk '
        {
            line=$0
            gsub(/^[[:space:]]+|[[:space:]]+$/, "", line)
            if (line == "") next
            if (line ~ /^#/) next
            if (line ~ /^```/) next
            if (line ~ /^---$/) next
            if (line ~ /^>/) {
                sub(/^>[[:space:]]*/, "", line)
                if (line != "") { print line; exit }
            }
            if (line ~ /^[0-9]+\.[[:space:]]+/) {
                sub(/^[0-9]+\.[[:space:]]+/, "", line)
                if (line != "") { print line; exit }
            }
            if (line ~ /^[-*][[:space:]]+/) {
                sub(/^[-*][[:space:]]+/, "", line)
                if (line != "") { print line; exit }
            }
            print line
            exit
        }
    ' "$file"
}

classify_category() {
    local title_lc="$1"
    local summary_lc="$2"

    if [[ "$title_lc" == *"cli"* || "$title_lc" == *"lancedb"* ]]; then
        echo "Rust"
        return
    fi

    if [[ "$title_lc" == *"github pages"* || "$title_lc" == *"部署"* || "$title_lc" == *"路由"* || "$summary_lc" == *"部署"* ]]; then
        echo "DevOps"
        return
    fi

    echo "Web"
}

category_description() {
    case "$1" in
        Rust)
            echo "Rust CLI、LanceDB 数据建模与检索实践文档"
            ;;
        DevOps)
            echo "部署、路由与构建流程相关工程实践文档"
            ;;
        Web)
            echo "前端渲染、交互与样式工程实践文档"
            ;;
        *)
            echo "通用工程实践文档"
            ;;
    esac
}

generate_tags() {
    local title_lc="$1"
    local summary_lc="$2"
    local tags=()

    [[ "$title_lc" == *"cli"* || "$summary_lc" == *"cli"* ]] && tags+=(cli)
    [[ "$title_lc" == *"lancedb"* || "$summary_lc" == *"lancedb"* ]] && tags+=(lancedb)
    [[ "$title_lc" == *"前端"* || "$summary_lc" == *"前端"* ]] && tags+=(frontend)
    [[ "$title_lc" == *"wasm"* || "$summary_lc" == *"wasm"* ]] && tags+=(wasm)
    [[ "$title_lc" == *"yew"* || "$summary_lc" == *"yew"* ]] && tags+=(yew)
    [[ "$title_lc" == *"markdown"* || "$summary_lc" == *"markdown"* ]] && tags+=(markdown)
    [[ "$title_lc" == *"mermaid"* || "$summary_lc" == *"mermaid"* ]] && tags+=(mermaid)
    [[ "$title_lc" == *"tailwind"* || "$summary_lc" == *"tailwind"* ]] && tags+=(tailwind)
    [[ "$title_lc" == *"github pages"* || "$summary_lc" == *"github pages"* ]] && tags+=(github-pages)
    [[ "$title_lc" == *"路由"* || "$summary_lc" == *"路由"* ]] && tags+=(routing)
    [[ "$title_lc" == *"部署"* || "$summary_lc" == *"部署"* ]] && tags+=(deployment)
    [[ "$title_lc" == *"图表"* || "$summary_lc" == *"图表"* ]] && tags+=(diagram)
    [[ "$title_lc" == *"复制"* || "$summary_lc" == *"复制"* ]] && tags+=(copy)
    [[ "$title_lc" == *"动画"* || "$title_lc" == *"旋转"* || "$summary_lc" == *"动画"* ]] && tags+=(animation)
    [[ "$title_lc" == *"toc"* || "$title_lc" == *"目录"* || "$summary_lc" == *"目录"* ]] && tags+=(toc)

    if [[ ${#tags[@]} -eq 0 ]]; then
        tags+=(notes)
    fi

    local out=()
    local seen=","
    for tag in "${tags[@]}"; do
        if [[ "$seen" != *",$tag,"* ]]; then
            out+=("$tag")
            seen+="$tag,"
        fi
    done

    local joined
    joined=$(IFS=,; echo "${out[*]}")
    echo "$joined"
}

step "Initializing database"
"$CLI_BIN" init --db-path "$DB_PATH"

step "Generating metadata from docs content"
printf "file\tcategory\tcategory_description\ttags\tsummary\n" > "$META_FILE"
while IFS= read -r file; do
    title=$(extract_title "$file")
    summary=$(extract_summary "$file")
    summary=${summary:-"文档概述"}

    title_lc=$(printf "%s" "$title" | tr '[:upper:]' '[:lower:]')
    summary_lc=$(printf "%s" "$summary" | tr '[:upper:]' '[:lower:]')

    category=$(classify_category "$title_lc" "$summary_lc")
    cat_desc=$(category_description "$category")
    tags=$(generate_tags "$title_lc" "$summary_lc")

    summary=$(printf "%s" "$summary" | tr '\t' ' ' | tr '\n' ' ')
    printf "%s\t%s\t%s\t%s\t%s\n" "$file" "$category" "$cat_desc" "$tags" "$summary" >> "$META_FILE"
done < <(rg --files docs -g '*.md' | sort)

doc_count=$(($(wc -l < "$META_FILE") - 1))
((doc_count > 0)) || fail "no markdown files found in docs/"

step "Writing docs into articles table"
while IFS=$'\t' read -r file category cat_desc tags summary; do
    [[ "$file" == "file" ]] && continue
    id=$(basename "$file" .md | tr '.' '-' | tr '[:upper:]' '[:lower:]')
    "$CLI_BIN" write-article \
        --db-path "$DB_PATH" \
        --file "$file" \
        --id "$id" \
        --summary "$summary" \
        --tags "$tags" \
        --category "$category" \
        --category-description "$cat_desc" \
        --language zh
done < "$META_FILE"

"$CLI_BIN" api --db-path "$DB_PATH" list-articles > "$OUT_DIR/list_articles.json"
article_total=$(jq -r '.total' "$OUT_DIR/list_articles.json")
assert_eq "$doc_count" "$article_total" "article total mismatch after write-article"

"$CLI_BIN" api --db-path "$DB_PATH" list-categories > "$OUT_DIR/list_categories_after_write.json"
jq -e '.categories | length > 0 and all(.[]; (.description | type == "string") and (.description | length > 0))' "$OUT_DIR/list_categories_after_write.json" >/dev/null \
    || fail "category descriptions should be non-empty"

step "Writing image directory into images table"
"$CLI_BIN" write-images --db-path "$DB_PATH" --dir ./content/images --recursive --generate-thumbnail

"$CLI_BIN" api --db-path "$DB_PATH" list-images > "$OUT_DIR/list_images_after_write.json"
image_total=$(jq -r '.total' "$OUT_DIR/list_images_after_write.json")
expected_images=$(rg --files content/images | wc -l | tr -d ' ')
assert_eq "$expected_images" "$image_total" "image total mismatch after write-images"

step "Building sync-notes dataset from docs"
while IFS=$'\t' read -r file category cat_desc tags summary; do
    [[ "$file" == "file" ]] && continue
    base=$(basename "$file")
    out="$NOTES_DIR/$base"
    title=$(extract_title "$file")
    tags_yaml=$(echo "$tags" | awk -F',' '{for(i=1;i<=NF;i++){printf("\"%s\"",$i); if(i<NF) printf(", ")}}')

    cat > "$out" <<NOTE
---
title: "$title"
summary: "$summary"
tags: [$tags_yaml]
category: "$category"
category_description: "$cat_desc"
author: "boliu"
date: "2026-02-10"
featured_image: "../../../content/images/wallhaven-e8xrjl.png"
read_time: 5
---

NOTE
    cat "$file" >> "$out"
    printf '\n\n![local-sample](../../../content/images/wallhaven-k88816.jpg)\n' >> "$out"
done < "$META_FILE"

step "Syncing notes directory"
"$CLI_BIN" sync-notes --db-path "$DB_PATH" --dir "$NOTES_DIR" --recursive --generate-thumbnail --language zh --default-category Notes --default-author boliu

"$CLI_BIN" api --db-path "$DB_PATH" list-articles > "$OUT_DIR/list_articles_after_sync.json"
article_total_after_sync=$(jq -r '.total' "$OUT_DIR/list_articles_after_sync.json")
assert_eq "$doc_count" "$article_total_after_sync" "article total mismatch after sync-notes"

"$CLI_BIN" api --db-path "$DB_PATH" get-article copy-feature > "$OUT_DIR/get_article_copy_feature.json"
jq -e '.featured_image | test("^images/[0-9a-f]{64}$")' "$OUT_DIR/get_article_copy_feature.json" >/dev/null \
    || fail "featured_image should be rewritten to images/<sha256>"
jq -e '.content | test("!\\[local-sample\\]\\(images/[0-9a-f]{64}\\)")' "$OUT_DIR/get_article_copy_feature.json" >/dev/null \
    || fail "markdown local image link was not rewritten"

step "Running top-level query and ensure-indexes"
"$CLI_BIN" query --db-path "$DB_PATH" --table articles --columns id,title,category --limit 3
"$CLI_BIN" query --db-path "$DB_PATH" --table articles --columns id,title --limit 1 --format vertical
"$CLI_BIN" ensure-indexes --db-path "$DB_PATH"

step "Running DB management commands"
"$CLI_BIN" db --db-path "$DB_PATH" list-tables
"$CLI_BIN" db --db-path "$DB_PATH" describe-table articles
"$CLI_BIN" db --db-path "$DB_PATH" describe-table images
"$CLI_BIN" db --db-path "$DB_PATH" describe-table taxonomies
"$CLI_BIN" db --db-path "$DB_PATH" count-rows articles --where "category='Web'"
"$CLI_BIN" db --db-path "$DB_PATH" query-rows articles --where "category='DevOps'" --columns id,title,category,tags --limit 5
"$CLI_BIN" db --db-path "$DB_PATH" query-rows taxonomies --where "kind='category'" --columns key,name,description --format vertical

"$CLI_BIN" db --db-path "$DB_PATH" update-rows articles --set "summary='e2e summary updated'" --where "id='copy-feature'"
"$CLI_BIN" api --db-path "$DB_PATH" get-article copy-feature > "$OUT_DIR/get_article_after_update.json"
jq -e '.summary == "e2e summary updated"' "$OUT_DIR/get_article_after_update.json" >/dev/null \
    || fail "update-rows did not persist summary change"

set +e
"$CLI_BIN" db --db-path "$DB_PATH" update-rows articles --set "summary='oops'" > "$OUT_DIR/guard_update.log" 2>&1
update_guard_code=$?
"$CLI_BIN" db --db-path "$DB_PATH" delete-rows articles > "$OUT_DIR/guard_delete.log" 2>&1
delete_guard_code=$?
set -e

[[ "$update_guard_code" -ne 0 ]] || fail "update guard should fail without --where/--all"
[[ "$delete_guard_code" -ne 0 ]] || fail "delete guard should fail without --where/--all"
grep -q "blocked" "$OUT_DIR/guard_update.log" || fail "missing guard message for update"
grep -q "blocked" "$OUT_DIR/guard_delete.log" || fail "missing guard message for delete"

"$CLI_BIN" db --db-path "$DB_PATH" upsert-article --json '{"id":"e2e-temp-article","title":"E2E Temp Article","content":"# temp","summary":"temp summary","tags":["temp","e2e"],"category":"Rust","author":"tester","date":"2026-02-10","featured_image":null,"read_time":1,"vector_en":null,"vector_zh":null,"created_at":1739160000000,"updated_at":1739160000000}'
"$CLI_BIN" api --db-path "$DB_PATH" get-article e2e-temp-article > "$OUT_DIR/get_temp_article.json"
jq -e '.id == "e2e-temp-article"' "$OUT_DIR/get_temp_article.json" >/dev/null || fail "upsert-article failed"
"$CLI_BIN" db --db-path "$DB_PATH" delete-rows articles --where "id='e2e-temp-article'"

temp_vec=$(python3 - <<'PY'
print('[' + ','.join(['0.001']*512) + ']')
PY
)
"$CLI_BIN" db --db-path "$DB_PATH" upsert-image --json "{\"id\":\"e2e-temp-image\",\"filename\":\"temp.bin\",\"data\":[1,2,3,4],\"thumbnail\":null,\"vector\":$temp_vec,\"metadata\":\"{\\\"source\\\":\\\"e2e\\\"}\",\"created_at\":1739160000000}"
"$CLI_BIN" api --db-path "$DB_PATH" list-images > "$OUT_DIR/list_images_after_temp.json"
jq -e '.images | any(.id == "e2e-temp-image")' "$OUT_DIR/list_images_after_temp.json" >/dev/null || fail "upsert-image failed"
"$CLI_BIN" db --db-path "$DB_PATH" delete-rows images --where "id='e2e-temp-image'"
"$CLI_BIN" api --db-path "$DB_PATH" list-images > "$OUT_DIR/list_images_after_temp_delete.json"
jq -e '.images | any(.id == "e2e-temp-image") | not' "$OUT_DIR/list_images_after_temp_delete.json" >/dev/null || fail "delete image row failed"

"$CLI_BIN" db --db-path "$DB_PATH" ensure-indexes
"$CLI_BIN" db --db-path "$DB_PATH" list-indexes articles --with-stats
"$CLI_BIN" db --db-path "$DB_PATH" drop-index articles content_idx
"$CLI_BIN" db --db-path "$DB_PATH" list-indexes articles
"$CLI_BIN" db --db-path "$DB_PATH" ensure-indexes --table articles
"$CLI_BIN" db --db-path "$DB_PATH" list-indexes articles
"$CLI_BIN" db --db-path "$DB_PATH" optimize articles
"$CLI_BIN" db --db-path "$DB_PATH" optimize images --all

"$CLI_BIN" db --db-path "$DB_PATH" create-table taxonomies --replace
"$CLI_BIN" db --db-path "$DB_PATH" count-rows taxonomies
"$CLI_BIN" db --db-path "$DB_PATH" drop-table taxonomies --yes
"$CLI_BIN" db --db-path "$DB_PATH" create-table taxonomies
"$CLI_BIN" sync-notes --db-path "$DB_PATH" --dir "$NOTES_DIR" --recursive --generate-thumbnail --language zh --default-category Notes --default-author boliu

step "Running API command set"
"$CLI_BIN" api --db-path "$DB_PATH" list-articles --category Web > "$OUT_DIR/api_list_articles_web.json"
"$CLI_BIN" api --db-path "$DB_PATH" list-articles --tag mermaid > "$OUT_DIR/api_list_articles_tag.json"
"$CLI_BIN" api --db-path "$DB_PATH" get-article copy-feature > "$OUT_DIR/api_get_article.json"
"$CLI_BIN" api --db-path "$DB_PATH" related-articles copy-feature > "$OUT_DIR/api_related.json"
"$CLI_BIN" api --db-path "$DB_PATH" search --q "Mermaid 图表" > "$OUT_DIR/api_search.json"
"$CLI_BIN" api --db-path "$DB_PATH" semantic-search --q "前端 渲染" > "$OUT_DIR/api_semantic.json"
"$CLI_BIN" api --db-path "$DB_PATH" list-tags > "$OUT_DIR/api_tags.json"
"$CLI_BIN" api --db-path "$DB_PATH" list-categories > "$OUT_DIR/api_categories.json"
"$CLI_BIN" api --db-path "$DB_PATH" list-images > "$OUT_DIR/api_images.json"

first_image_id=$(jq -r '.images[0].id' "$OUT_DIR/api_images.json")
[[ "$first_image_id" != "null" ]] || fail "no image found for search-images/get-image"
"$CLI_BIN" api --db-path "$DB_PATH" search-images --id "$first_image_id" > "$OUT_DIR/api_search_images.json"
"$CLI_BIN" api --db-path "$DB_PATH" get-image "$first_image_id" --thumb --out "$OUT_DIR/api_thumb.bin" > "$OUT_DIR/api_get_image.json"

jq -e '.total > 0' "$OUT_DIR/api_list_articles_web.json" >/dev/null || fail "api list-articles --category returned empty"
jq -e '.total > 0' "$OUT_DIR/api_list_articles_tag.json" >/dev/null || fail "api list-articles --tag returned empty"
jq -e '.id == "copy-feature"' "$OUT_DIR/api_get_article.json" >/dev/null || fail "api get-article failed"
jq -e '.total >= 0' "$OUT_DIR/api_related.json" >/dev/null || fail "api related-articles invalid"
jq -e '.total > 0' "$OUT_DIR/api_search.json" >/dev/null || fail "api search returned empty"
jq -e '.total > 0' "$OUT_DIR/api_semantic.json" >/dev/null || fail "api semantic-search returned empty"
jq -e '.tags | length > 0' "$OUT_DIR/api_tags.json" >/dev/null || fail "api list-tags returned empty"
jq -e '.categories | length > 0 and all(.[]; .description | length > 0)' "$OUT_DIR/api_categories.json" >/dev/null || fail "api list-categories descriptions invalid"
jq -e '.total > 0' "$OUT_DIR/api_images.json" >/dev/null || fail "api list-images returned empty"
jq -e '.total >= 0' "$OUT_DIR/api_search_images.json" >/dev/null || fail "api search-images invalid"
jq -e '.bytes > 0 and (.output | length > 0)' "$OUT_DIR/api_get_image.json" >/dev/null || fail "api get-image invalid"
[[ -s "$OUT_DIR/api_thumb.bin" ]] || fail "api get-image output file is empty"

step "Validating friendly error outputs"
set +e
"$CLI_BIN" db --db-path "$DB_PATH" query-rows articles --columns no_such_column > "$OUT_DIR/err_bad_column.log" 2>&1
bad_column_code=$?
"$CLI_BIN" db --db-path "$DB_PATH" query-rows no_such_table --limit 1 > "$OUT_DIR/err_bad_table.log" 2>&1
bad_table_code=$?
set -e

[[ "$bad_column_code" -ne 0 ]] || fail "invalid column should fail"
[[ "$bad_table_code" -ne 0 ]] || fail "invalid table should fail"
grep -q "Schema columns" "$OUT_DIR/err_bad_column.log" || fail "invalid column message should include schema columns"
grep -q "Available tables" "$OUT_DIR/err_bad_table.log" || fail "invalid table message should include available tables"

step "All CLI checks passed"
echo "Workspace: $WORKDIR"
echo "Database:  $DB_PATH"
