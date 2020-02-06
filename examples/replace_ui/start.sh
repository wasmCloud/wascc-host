#!/usr/bin/env bash 

set -euo pipefail

DIR=$(pwd)

sed "s,DIR,$DIR," < nginx.template > nginx.conf

# npm run build # Only necessary if you've lost your static build files.

cat $DIR/nginx.conf

nginx -c $DIR/nginx.conf