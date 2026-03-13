#!/bin/sh

# Copyright (c) 2024 RapidStream Design Automation, Inc. and contributors.
# All rights reserved. The contributor(s) of this file has/have agreed to the
# RapidStream Contributor License Agreement.

# This script is used to install TAPA from a local package on the target machine.
#
# Usage:
#   TAPA_LOCAL_PACKAGE=./path/to/tapa.tar.gz ./install.sh
#
# If the user runs this script with the root privilege, it will install the software
# in the /opt/tapa directory. It further creates symbolic links in the
# /usr/local/bin directory to the executables in the /opt/tapa directory to
# make the software available in the system path.
#
# Otherwise, if the user runs this script without the root privilege, it will install
# the software in the $HOME/.tapa directory. It further modifies the user's
# PATH environment variable to include the $HOME/.tapa directory.

# Treat unset variables as an error when substituting. And exit immediately if a
# pipeline returns non-zero status.
set -ue

# Default values for the installation options.
# Support both new and legacy environment variable names.
TAPA_LOCAL_PACKAGE="${TAPA_LOCAL_PACKAGE:-${RAPIDSTREAM_LOCAL_PACKAGE:-}}"

if [ -z "$TAPA_LOCAL_PACKAGE" ]; then
  echo "Error: TAPA_LOCAL_PACKAGE must be set to the path of the TAPA tarball."
  echo ""
  echo "To build TAPA from source and create a tarball, run:"
  echo "  bazel build --config=release //:tapa-pkg-tar"
  echo ""
  echo "Then install with:"
  echo "  TAPA_LOCAL_PACKAGE=./bazel-bin/tapa-pkg-tar.tar ./install.sh"
  exit 1
fi

if [ "$(id -u)" -eq 0 ]; then
  # Default to /opt/tapa if the user has the root privilege.
  TAPA_INSTALL_DIR="${TAPA_INSTALL_DIR:-${RAPIDSTREAM_INSTALL_DIR:-/opt/tapa}}"
  CREATE_SYMLINKS="yes"
  MODIFY_PROFILE_PATH="no"

elif [ "$(id -u)" -ne 0 ]; then
  # Default to the user's home directory if the user does not have the root privilege.
  TAPA_INSTALL_DIR="${TAPA_INSTALL_DIR:-${RAPIDSTREAM_INSTALL_DIR:-$HOME/.tapa}}"
  CREATE_SYMLINKS="no"
  MODIFY_PROFILE_PATH="yes"

fi

VERBOSE="${VERBOSE:-yes}"
QUIET="${QUIET:-no}"

# Display the usage of this script.
usage() {
  cat <<EOF
install.sh - Install TAPA from a local package.

Usage: TAPA_LOCAL_PACKAGE=./tapa.tar.gz ./install.sh [OPTIONS]

Options:
  -t, --target <directory>     Specify the directory to install the software to.
      --no-create-symlinks     Do not create symbolic links in the system path.
      --no-modify-path         Do not modify the PATH environment variable.

  -q, --quiet                  Disable verbose output.
  -qq, --quiet-all             Disable most of the output.

  -h, --help                   Display this help message and exit.
EOF
}

# Main function of this script.
main() {
  # Parse the command-line arguments.
  parse_args "$@"

  # Display the installation options if the verbose mode is enabled.
  if [ "$VERBOSE" = "yes" ]; then
    echo "Please verify the specified installation options:"
    echo "  Local package:     $TAPA_LOCAL_PACKAGE"
    echo "  Install target:    $TAPA_INSTALL_DIR"
    echo "  Create symlinks:   $CREATE_SYMLINKS"
    echo "  Modify PATH:       $MODIFY_PROFILE_PATH"
    printf "Press Enter to continue, or Ctrl+C to cancel..."
    read -r _
  fi

  # Check if the installation directory exists.
  check_install_dir

  # Display the installation message.
  if [ "$QUIET" = "no" ]; then
    echo "Installing TAPA to \"$TAPA_INSTALL_DIR\"..."
  fi

  # Extract the TAPA package.
  extract_tapa_package

  # Create symbolic links in the system path.
  create_symlinks

  # Modify the PATH environment variable.
  modify_profile_path
}

# Parse the command-line arguments.
parse_args() {
  while [ $# -gt 0 ]; do
    case "$1" in
    -t | --target)
      TAPA_INSTALL_DIR="$2"
      shift 2
      ;;
    --target=*)
      TAPA_INSTALL_DIR="${1#*=}"
      shift
      ;;
    --no-create-symlinks)
      CREATE_SYMLINKS="no"
      shift
      ;;
    --no-modify-path)
      MODIFY_PROFILE_PATH="no"
      shift
      ;;
    -q | --quiet)
      VERBOSE="no"
      shift
      ;;
    -qq | --quiet-all)
      VERBOSE="no"
      QUIET="yes"
      shift
      ;;
    -h | --help)
      usage
      exit 0
      ;;
    *)
      echo "Unknown option: $1"
      usage
      exit 1
      ;;
    esac
  done

  # Verify the options.
  if [ "$VERBOSE" = "yes" ] && [ "$QUIET" = "yes" ]; then
    echo "Error: The options '-v' and '-q' cannot be used together."
    exit 1
  fi
}

# Check if the installation directory exists. If so, prompt the user to confirm.
check_install_dir() {
  # If the installation directory exists
  if [ -d "$TAPA_INSTALL_DIR" ]; then

    # If the user does not enable the auto-confirm option
    if [ "$VERBOSE" = "yes" ]; then
      # Prompt the user to confirm
      printf "The installation directory already exists. Do you want to overwrite it? [y/N]: "
      read -r answer

      # If the user does not confirm
      if [ "$answer" != "y" ] && [ "$answer" != "Y" ]; then
        # Abort the installation
        echo "Aborted. No changes were made."
        exit 1
      fi
    fi

    # If the user enables the auto-confirm option or confirms the prompt,
    # show the message that the installation directory will be overwritten.
    if [ "$QUIET" = "no" ]; then
      echo "Overwriting the installation directory: \"$TAPA_INSTALL_DIR\"..."
    fi

    # Remove the existing installation directory
    rm -rf "$TAPA_INSTALL_DIR"
  fi
}

# Extract the TAPA package from the local tarball.
extract_tapa_package() {
  if [ ! -f "$TAPA_LOCAL_PACKAGE" ]; then
    echo "Error: Local package not found: \"$TAPA_LOCAL_PACKAGE\""
    exit 1
  fi

  # Extract the TAPA package.
  if [ "$VERBOSE" = "yes" ]; then
    echo "Extracting TAPA from: \"$TAPA_LOCAL_PACKAGE\" to: \"$TAPA_INSTALL_DIR\"..."
  elif [ "$QUIET" = "no" ]; then
    echo "Extracting TAPA..."
  fi
  mkdir -p "$TAPA_INSTALL_DIR"
  tar -xzf "$TAPA_LOCAL_PACKAGE" -C "$TAPA_INSTALL_DIR" --overwrite
}

# Create symbolic links in the system path.
create_symlinks() {
  if [ "$CREATE_SYMLINKS" = "yes" ]; then
    if [ "$QUIET" = "no" ]; then
      echo "Creating symbolic links in the system path \"/usr/local/bin\"..."
    fi

    # Create symbolic links for each executable in the installation directory.
    for bin in "$TAPA_INSTALL_DIR"/usr/bin/*; do
      # Skip the directories.
      if [ ! -f "$bin" ]; then
        continue
      fi

      bin_name="$(basename "$bin")"
      if [ "$VERBOSE" = "yes" ]; then
        echo "Creating symbolic link: \"/usr/local/bin/$bin_name\" -> \"$bin\"..."
      fi
      ln -sf "$bin" "/usr/local/bin/$bin_name"
    done
  fi
}

modify_profile_path_in_file() {
  profile_file="$1"

  # Check if the profile file exists.
  if [ ! -f "$profile_file" ]; then
    if [ "$VERBOSE" = "yes" ]; then
      echo "The profile file \"$profile_file\" does not exist. Skipping..."
    fi
    return
  fi

  # Check if the PATH environment variable is already modified.
  if grep -q "$TAPA_INSTALL_DIR" "$profile_file"; then
    if [ "$VERBOSE" = "yes" ]; then
      echo "The PATH to TAPA is already set in \"$profile_file\". Skipping..."
    fi
    return
  fi

  # Add the PATH environment variable to the profile file.
  if [ "$QUIET" = "no" ]; then
    echo "Adding PATH to TAPA to \"$profile_file\"..."
  fi
  echo "export PATH=\"\$PATH:$TAPA_INSTALL_DIR/usr/bin\"" >>"$profile_file"
}

# Modify the PATH environment variable.
modify_profile_path() {
  if [ "$MODIFY_PROFILE_PATH" = "yes" ]; then
    if [ "$VERBOSE" = "yes" ]; then
      echo "Modifying the PATH environment variable..."
    fi

    # Modify the PATH environment variable in the user's profile files.
    modify_profile_path_in_file "$HOME/.profile"
    modify_profile_path_in_file "$HOME/.bashrc"
    modify_profile_path_in_file "$HOME/.bash_profile"
    modify_profile_path_in_file "$HOME/.zshrc"
    modify_profile_path_in_file "$HOME/.zprofile"

    if [ "$QUIET" = "no" ]; then
      echo "Please restart your shell to finish the installation."
      echo "Alternatively, you can run the following command to apply the changes:"
      echo "  export PATH=\"\$PATH:$TAPA_INSTALL_DIR/usr/bin\""
    fi
  fi
}

main "$@"
