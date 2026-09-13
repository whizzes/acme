#!/usr/bin/env bash
set -ex

readonly PROGDIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )" &> /dev/null && pwd )"

# Package Acme into Docker image
#
# PARAMS:
# $1: The tag to build this Docker image with
#       Ex: 0.7.4
# $2: The rust target to build the Docker image for e.g. x86_64-unknown-linux-musl
# $3: The path to the acme executable
#       Ex: target/x86_64-unknown-linux-musl/release/acme
#
# OUTPUTS:
# Docker image with tag: acme:$1-$2
main() {
  local -r tag=$1; shift
  local -r target=$1; shift
  local -r acme=$1; shift
  local -r tmp_dir=$(mktemp -d -t acme-docker-image-XXXXXX)
  local -r docker_repo="acme"
  local build_args

  echo "Building Docker image for $target with tag $tag"

  cp "${acme}" "${tmp_dir}/acme"
  chmod +x "${tmp_dir}/acme"
  cp "${PROGDIR}/Dockerfile" "${tmp_dir}/Dockerfile"

  if [ "$target" = "aarch64-unknown-linux-musl" ]; then
    local build_args="--build-arg ARCH=arm64v8/"
  fi

  pushd "${tmp_dir}"
  docker build -t "$docker_repo:$tag-$target" $build_args .
}

main "$@"
