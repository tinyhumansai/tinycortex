.PHONY: all setup build clean arxiv markdown figures-png

all: markdown

setup:
	./scripts/setup.sh

figures-png:
	./scripts/export-svgs-to-png.sh

build:
	@echo "PDF build is disabled for now. Use 'make markdown' to render paper/README.md."

markdown:
	./scripts/render_markdown.sh
	echo "README updated: ./README.md"

clean:
	./scripts/clean.sh

arxiv:
	./scripts/package_arxiv.sh
