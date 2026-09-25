"""Quest Pro stereo tongue architectures from Qpro-Enhanced-FT (MIT license).

Source: https://github.com/n0tmast3r/Qpro-Enhanced-FT, train_tongue_model.py.
Only the inference model definitions are included here.
"""

import torch
from torch import nn
from torch.nn import functional as F


SIGNED_TARGETS = {"horizontal", "vertical", "twist"}


class TongueCameraEncoder(nn.Module):
    def __init__(self) -> None:
        super().__init__()
        channels = (1, 24, 40, 64, 96)
        layers = []
        for input_channels, output_channels in zip(channels, channels[1:]):
            layers.extend([
                nn.Conv2d(input_channels, output_channels, 3, stride=2, padding=1, bias=False),
                nn.BatchNorm2d(output_channels),
                nn.SiLU(inplace=True),
            ])
        layers.append(nn.AdaptiveAvgPool2d(1))
        self.network = nn.Sequential(*layers)

    def forward(self, image: torch.Tensor) -> torch.Tensor:
        return self.network(image).flatten(1)


class StereoTongueModel(nn.Module):
    def __init__(self, target_names: list[str]) -> None:
        super().__init__()
        self.encoder = TongueCameraEncoder()
        self.fusion = nn.Sequential(
            nn.Linear(96 * 4, 256), nn.SiLU(inplace=True), nn.Dropout(0.12),
            nn.Linear(256, 128), nn.SiLU(inplace=True),
            nn.Linear(128, len(target_names)),
        )
        self.register_buffer("signed_mask", torch.tensor(
            [name in SIGNED_TARGETS for name in target_names], dtype=torch.bool
        ))

    def forward(self, cameras: torch.Tensor) -> torch.Tensor:
        left = self.encoder(cameras[:, 0:1])
        right = self.encoder(cameras[:, 1:2])
        fused = torch.cat((left, right, torch.abs(left - right), left * right), dim=1)
        logits = self.fusion(fused)
        return torch.where(self.signed_mask, torch.tanh(logits), torch.sigmoid(logits))


class ResidualBlock(nn.Module):
    def __init__(self, channels: int) -> None:
        super().__init__()
        self.network = nn.Sequential(
            nn.Conv2d(channels, channels, 3, padding=1, bias=False),
            nn.BatchNorm2d(channels), nn.SiLU(inplace=True),
            nn.Conv2d(channels, channels, 3, padding=1, bias=False),
            nn.BatchNorm2d(channels),
        )

    def forward(self, values: torch.Tensor) -> torch.Tensor:
        return F.silu(values + self.network(values), inplace=True)


class SpatialTongueEncoder(nn.Module):
    def __init__(self) -> None:
        super().__init__()
        layers = [
            nn.Conv2d(1, 32, 5, stride=2, padding=2, bias=False),
            nn.BatchNorm2d(32), nn.SiLU(inplace=True), ResidualBlock(32),
        ]
        for input_channels, output_channels in ((32, 64), (64, 96), (96, 160)):
            layers.extend([
                nn.Conv2d(input_channels, output_channels, 3, stride=2, padding=1, bias=False),
                nn.BatchNorm2d(output_channels), nn.SiLU(inplace=True), ResidualBlock(output_channels),
            ])
        self.network = nn.Sequential(*layers)

    def forward(self, image: torch.Tensor) -> torch.Tensor:
        return self.network(image)


class SpatialStereoTongueModel(nn.Module):
    def __init__(self, target_names: list[str]) -> None:
        super().__init__()
        self.encoder = SpatialTongueEncoder()
        self.stereo_fusion = nn.Sequential(
            nn.Conv2d(160 * 4, 224, 1, bias=False), nn.BatchNorm2d(224),
            nn.SiLU(inplace=True), ResidualBlock(224),
            nn.Conv2d(224, 256, 3, stride=2, padding=1, bias=False),
            nn.BatchNorm2d(256), nn.SiLU(inplace=True), ResidualBlock(256),
        )
        self.head = nn.Sequential(
            nn.Linear(256 * 2, 384), nn.SiLU(inplace=True), nn.Dropout(0.18),
            nn.Linear(384, 192), nn.SiLU(inplace=True), nn.Dropout(0.08),
            nn.Linear(192, len(target_names)),
        )
        self.register_buffer("signed_mask", torch.tensor(
            [name in SIGNED_TARGETS for name in target_names], dtype=torch.bool
        ))

    def forward(self, cameras: torch.Tensor) -> torch.Tensor:
        left = self.encoder(cameras[:, 0:1])
        right = self.encoder(cameras[:, 1:2])
        stereo = torch.cat((left, right, torch.abs(left - right), left * right), dim=1)
        fused = self.stereo_fusion(stereo)
        pooled = torch.cat((
            F.adaptive_avg_pool2d(fused, 1).flatten(1),
            F.adaptive_max_pool2d(fused, 1).flatten(1),
        ), dim=1)
        logits = self.head(pooled)
        return torch.where(self.signed_mask, torch.tanh(logits), torch.sigmoid(logits))


def create_model(architecture: str, target_names: list[str]) -> nn.Module:
    if architecture == "legacy-late-fusion-v1":
        return StereoTongueModel(target_names)
    if architecture == "spatial-stereo-resnet-v2":
        return SpatialStereoTongueModel(target_names)
    raise ValueError(f"Unknown tongue model architecture: {architecture}")
