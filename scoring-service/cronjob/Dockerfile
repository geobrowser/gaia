FROM python:3.13-slim

WORKDIR /app

# Install uv
COPY --from=ghcr.io/astral-sh/uv:0.9.15 /uv /usr/local/bin/uv

# Copy application files
COPY . .

# Install dependencies
RUN uv sync --frozen

# Run the application
CMD ["uv", "run", "python", "main.py"]
