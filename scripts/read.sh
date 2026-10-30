#!/bin/bash
# 打开阅读器（主机侧渲染，推送到自托管服务端）
#
#   ./scripts/read.sh books/示例书.txt --serve [--sync http://<服务器>:12900]
#   ./scripts/read.sh books/示例书.txt --dry-run      # 只渲染首页预览
#
# 服务端地址也可用环境变量 NOTE4_SERVER 提供：
#   NOTE4_SERVER=http://<服务器>:12900 ./scripts/read.sh books/示例书.txt --serve
set -e
cd "$(dirname "$0")/../reader"

if [ $# -lt 1 ]; then
  echo "用法: $0 <书.txt> [--serve|--dry-run] [--sync <url>] [--font 28] [--chapter N] [--page N] [--window N]" >&2
  exit 1
fi

BOOK="$1"; shift
if [ ! -f "$BOOK" ] && [ -f "../$BOOK" ]; then
  BOOK="../$BOOK"
fi

exec cargo run --release -- "$BOOK" --font 28 "$@"