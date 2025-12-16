FROM python:3.13-slim

WORKDIR /app

# Install uv
COPY --from=ghcr.io/astral-sh/uv:0.9.15 /uv /usr/local/bin/uv

# Copy scoring-service directory
COPY scoring-service ./scoring-service

WORKDIR /app/scoring-service

# Install dependencies
RUN uv sync --frozen

# Run the application
CMD ["uv", "run", "python", "main.py"]

