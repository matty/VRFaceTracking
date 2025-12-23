use anyhow::Result;
use common::{UnifiedExpressions, UnifiedTrackingData};
use rosc::{encoder, OscBundle, OscMessage, OscPacket, OscType};
use std::net::UdpSocket;

pub struct ResoniteOsc {
    socket: Option<UdpSocket>,
    target_addr: String,
}

impl ResoniteOsc {
    pub fn new(target_addr: &str) -> Self {
        Self {
            socket: None,
            target_addr: target_addr.to_string(),
        }
    }

    pub fn initialize(&mut self) -> Result<()> {
        let socket = UdpSocket::bind("0.0.0.0:0")?;
        self.socket = Some(socket);
        Ok(())
    }

    pub fn send(&self, data: &UnifiedTrackingData) -> Result<()> {
        let socket = self
            .socket
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("ResoniteOsc not initialized"))?;

        let mut messages = Vec::with_capacity(80);

        macro_rules! add_msg {
            ($addr:expr, $val:expr) => {
                messages.push(OscMessage {
                    addr: $addr.to_string(),
                    args: vec![OscType::Float($val)],
                });
            };
        }

        add_msg!("/avatar/parameters/LeftEyeX", data.eye.left.gaze.x);
        add_msg!("/avatar/parameters/LeftEyeY", data.eye.left.gaze.y);
        add_msg!("/avatar/parameters/RightEyeX", data.eye.right.gaze.x);
        add_msg!("/avatar/parameters/RightEyeY", data.eye.right.gaze.y);

        add_msg!("/avatar/parameters/LeftEyeLid", 1.0 - data.eye.left.openness);
        add_msg!("/avatar/parameters/RightEyeLid", 1.0 - data.eye.right.openness);

        add_msg!("/sl/xrfb/facew/EyesClosedL", 1.0 - data.eye.left.openness);
        add_msg!("/sl/xrfb/facew/EyesClosedR", 1.0 - data.eye.right.openness);

        let w = |expr: UnifiedExpressions| data.shapes[expr as usize].weight;

        add_msg!("/sl/xrfb/facew/JawDrop", w(UnifiedExpressions::JawOpen));
        add_msg!("/sl/xrfb/facew/JawSidewaysLeft", w(UnifiedExpressions::JawLeft));
        add_msg!("/sl/xrfb/facew/JawSidewaysRight", w(UnifiedExpressions::JawRight));
        add_msg!("/sl/xrfb/facew/JawThrust", w(UnifiedExpressions::JawForward));

        add_msg!("/sl/xrfb/facew/LipCornerPullerL", w(UnifiedExpressions::MouthCornerPullLeft));
        add_msg!("/sl/xrfb/facew/LipCornerPullerR", w(UnifiedExpressions::MouthCornerPullRight));
        add_msg!("/sl/xrfb/facew/LipCornerDepressorL", w(UnifiedExpressions::MouthFrownLeft));
        add_msg!("/sl/xrfb/facew/LipCornerDepressorR", w(UnifiedExpressions::MouthFrownRight));

        add_msg!("/sl/xrfb/facew/LipFunnelerLT", w(UnifiedExpressions::LipFunnelUpperLeft));
        add_msg!("/sl/xrfb/facew/LipFunnelerRT", w(UnifiedExpressions::LipFunnelUpperRight));
        add_msg!("/sl/xrfb/facew/LipFunnelerLB", w(UnifiedExpressions::LipFunnelLowerLeft));
        add_msg!("/sl/xrfb/facew/LipFunnelerRB", w(UnifiedExpressions::LipFunnelLowerRight));

        add_msg!("/sl/xrfb/facew/LipPuckerL", w(UnifiedExpressions::LipPuckerLowerLeft).max(w(UnifiedExpressions::LipPuckerUpperLeft)));
        add_msg!("/sl/xrfb/facew/LipPuckerR", w(UnifiedExpressions::LipPuckerLowerRight).max(w(UnifiedExpressions::LipPuckerUpperRight)));

        add_msg!("/sl/xrfb/facew/LipPressorL", w(UnifiedExpressions::MouthPressLeft));
        add_msg!("/sl/xrfb/facew/LipPressorR", w(UnifiedExpressions::MouthPressRight));

        add_msg!("/sl/xrfb/facew/LipSuckLT", w(UnifiedExpressions::LipSuckUpperLeft));
        add_msg!("/sl/xrfb/facew/LipSuckRT", w(UnifiedExpressions::LipSuckUpperRight));
        add_msg!("/sl/xrfb/facew/LipSuckLB", w(UnifiedExpressions::LipSuckLowerLeft));
        add_msg!("/sl/xrfb/facew/LipSuckRB", w(UnifiedExpressions::LipSuckLowerRight));

        add_msg!("/sl/xrfb/facew/LipTightenerL", w(UnifiedExpressions::MouthTightenerLeft));
        add_msg!("/sl/xrfb/facew/LipTightenerR", w(UnifiedExpressions::MouthTightenerRight));

        add_msg!("/sl/xrfb/facew/LipStretcherL", w(UnifiedExpressions::MouthStretchLeft));
        add_msg!("/sl/xrfb/facew/LipStretcherR", w(UnifiedExpressions::MouthStretchRight));

        add_msg!("/sl/xrfb/facew/UpperLipRaiserL", w(UnifiedExpressions::MouthUpperUpLeft));
        add_msg!("/sl/xrfb/facew/UpperLipRaiserR", w(UnifiedExpressions::MouthUpperUpRight));
        add_msg!("/sl/xrfb/facew/LowerLipDepressorL", w(UnifiedExpressions::MouthLowerDownLeft));
        add_msg!("/sl/xrfb/facew/LowerLipDepressorR", w(UnifiedExpressions::MouthLowerDownRight));

        add_msg!("/sl/xrfb/facew/MouthLeft", w(UnifiedExpressions::MouthUpperLeft).max(w(UnifiedExpressions::MouthLowerLeft)));
        add_msg!("/sl/xrfb/facew/MouthRight", w(UnifiedExpressions::MouthUpperRight).max(w(UnifiedExpressions::MouthLowerRight)));

        add_msg!("/sl/xrfb/facew/CheekPuffL", w(UnifiedExpressions::CheekPuffLeft));
        add_msg!("/sl/xrfb/facew/CheekPuffR", w(UnifiedExpressions::CheekPuffRight));
        add_msg!("/sl/xrfb/facew/CheekSuckL", w(UnifiedExpressions::CheekSuckLeft));
        add_msg!("/sl/xrfb/facew/CheekSuckR", w(UnifiedExpressions::CheekSuckRight));
        add_msg!("/sl/xrfb/facew/CheekRaiserL", w(UnifiedExpressions::CheekSquintLeft));
        add_msg!("/sl/xrfb/facew/CheekRaiserR", w(UnifiedExpressions::CheekSquintRight));

        add_msg!("/sl/xrfb/facew/BrowLowererL", w(UnifiedExpressions::BrowLowererLeft));
        add_msg!("/sl/xrfb/facew/BrowLowererR", w(UnifiedExpressions::BrowLowererRight));
        add_msg!("/sl/xrfb/facew/InnerBrowRaiserL", w(UnifiedExpressions::BrowInnerUpLeft));
        add_msg!("/sl/xrfb/facew/InnerBrowRaiserR", w(UnifiedExpressions::BrowInnerUpRight));
        add_msg!("/sl/xrfb/facew/OuterBrowRaiserL", w(UnifiedExpressions::BrowOuterUpLeft));
        add_msg!("/sl/xrfb/facew/OuterBrowRaiserR", w(UnifiedExpressions::BrowOuterUpRight));

        add_msg!("/sl/xrfb/facew/LidTightenerL", w(UnifiedExpressions::EyeSquintLeft));
        add_msg!("/sl/xrfb/facew/LidTightenerR", w(UnifiedExpressions::EyeSquintRight));
        add_msg!("/sl/xrfb/facew/UpperLidRaiserL", w(UnifiedExpressions::EyeWideLeft));
        add_msg!("/sl/xrfb/facew/UpperLidRaiserR", w(UnifiedExpressions::EyeWideRight));

        add_msg!("/sl/xrfb/facew/NoseWrinklerL", w(UnifiedExpressions::NoseSneerLeft));
        add_msg!("/sl/xrfb/facew/NoseWrinklerR", w(UnifiedExpressions::NoseSneerRight));
        add_msg!("/sl/xrfb/facew/ChinRaiserT", w(UnifiedExpressions::MouthRaiserUpper));
        add_msg!("/sl/xrfb/facew/ChinRaiserB", w(UnifiedExpressions::MouthRaiserLower));
        add_msg!("/sl/xrfb/facew/DimplerL", w(UnifiedExpressions::MouthDimpleLeft));
        add_msg!("/sl/xrfb/facew/DimplerR", w(UnifiedExpressions::MouthDimpleRight));

        add_msg!("/sl/xrfb/facew/TongueOut", w(UnifiedExpressions::TongueOut));
        add_msg!("/sl/xrfb/facew/TongueTipAlveolar", w(UnifiedExpressions::TongueUp));
        add_msg!("/sl/xrfb/facew/TongueRetreat", w(UnifiedExpressions::TongueDown));

        if messages.is_empty() {
            return Ok(());
        }

        let bundle = OscBundle {
            timetag: rosc::OscTime::from((0, 0)),
            content: messages.into_iter().map(OscPacket::Message).collect(),
        };

        let packet = OscPacket::Bundle(bundle);
        let msg_buf = encoder::encode(&packet)?;

        socket.send_to(&msg_buf, &self.target_addr)?;

        Ok(())
    }
}
