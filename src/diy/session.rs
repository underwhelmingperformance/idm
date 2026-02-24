use std::ops::BitOr;

use super::upload_runtime::{DiyError, DiyHandler, DiyModeGuard, RuntimeMode};
use crate::Rgb;
use crate::error::ProtocolError;
use crate::hw::DeviceSession;

/// Request payload for one RGB888 DIY frame upload.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct UploadRequest {
    frame: crate::Rgb888Frame,
}

impl UploadRequest {
    /// Creates a request from one RGB888 frame.
    ///
    /// ```
    /// # let dimensions = idm::PanelDimensions::new(1, 1).expect("valid panel");
    /// # let frame = idm::Rgb888Frame::try_from((dimensions, vec![0x01, 0x02, 0x03])).expect("valid frame");
    /// let request = idm::diy::UploadRequest::new(frame);
    /// ```
    #[must_use]
    pub fn new(frame: crate::Rgb888Frame) -> Self {
        Self { frame }
    }

    /// Returns the frame payload bytes.
    #[must_use]
    pub fn payload(&self) -> &[u8] {
        self.frame.payload()
    }

    /// Returns the frame payload.
    #[must_use]
    pub fn frame(&self) -> &crate::Rgb888Frame {
        &self.frame
    }
}

/// Stats returned after a successful DIY frame upload.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct UploadStats {
    pub(crate) bytes_written: usize,
    pub(crate) chunks_written: usize,
    pub(crate) logical_chunks_sent: usize,
}

impl UploadStats {
    /// Returns total bytes written.
    #[must_use]
    pub fn bytes_written(&self) -> usize {
        self.bytes_written
    }

    /// Returns number of transport chunks written.
    #[must_use]
    pub fn chunks_written(&self) -> usize {
        self.chunks_written
    }

    /// Returns number of logical chunks sent.
    #[must_use]
    pub fn logical_chunks_sent(&self) -> usize {
        self.logical_chunks_sent
    }
}

/// Stats returned after a DIY runtime command.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct CommandStats {
    bytes_written: usize,
}

impl CommandStats {
    /// Returns the number of bytes written by this command.
    #[must_use]
    pub fn bytes_written(&self) -> usize {
        self.bytes_written
    }
}

/// One pixel coordinate on the active panel.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct Point {
    x: u8,
    y: u8,
}

impl Point {
    /// Creates one point.
    ///
    /// ```
    /// use idm::diy::Point;
    ///
    /// let point = Point::new(3, 7);
    /// assert_eq!(3, point.x());
    /// assert_eq!(7, point.y());
    /// ```
    #[must_use]
    pub fn new(x: u8, y: u8) -> Self {
        Self { x, y }
    }

    /// Returns the x coordinate.
    ///
    /// ```
    /// use idm::diy::Point;
    ///
    /// let point = Point::new(1, 2);
    /// assert_eq!(1, point.x());
    /// ```
    #[must_use]
    pub fn x(self) -> u8 {
        self.x
    }

    /// Returns the y coordinate.
    ///
    /// ```
    /// use idm::diy::Point;
    ///
    /// let point = Point::new(1, 2);
    /// assert_eq!(2, point.y());
    /// ```
    #[must_use]
    pub fn y(self) -> u8 {
        self.y
    }
}

/// Four-direction shift flags for movement runtime commands.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
pub struct Shift {
    up: bool,
    down: bool,
    left: bool,
    right: bool,
}

impl Shift {
    /// Creates a shift with explicit direction flags.
    #[must_use]
    pub(crate) fn new(up: bool, down: bool, left: bool, right: bool) -> Self {
        Self {
            up,
            down,
            left,
            right,
        }
    }

    /// Returns a one-step upward shift.
    ///
    /// ```
    /// use idm::diy::Shift;
    ///
    /// let shift = Shift::up();
    /// assert!(shift.is_up());
    /// ```
    #[must_use]
    pub fn up() -> Self {
        Self::new(true, false, false, false)
    }

    /// Returns a one-step downward shift.
    ///
    /// ```
    /// use idm::diy::Shift;
    ///
    /// let shift = Shift::down();
    /// assert!(shift.is_down());
    /// ```
    #[must_use]
    pub fn down() -> Self {
        Self::new(false, true, false, false)
    }

    /// Returns a one-step left shift.
    ///
    /// ```
    /// use idm::diy::Shift;
    ///
    /// let shift = Shift::left();
    /// assert!(shift.is_left());
    /// ```
    #[must_use]
    pub fn left() -> Self {
        Self::new(false, false, true, false)
    }

    /// Returns a one-step right shift.
    ///
    /// ```
    /// use idm::diy::Shift;
    ///
    /// let shift = Shift::right();
    /// assert!(shift.is_right());
    /// ```
    #[must_use]
    pub fn right() -> Self {
        Self::new(false, false, false, true)
    }

    /// Returns whether upward movement is enabled.
    ///
    /// ```
    /// use idm::diy::Shift;
    ///
    /// assert!(Shift::up().is_up());
    /// ```
    #[must_use]
    pub fn is_up(self) -> bool {
        self.up
    }

    /// Returns whether downward movement is enabled.
    ///
    /// ```
    /// use idm::diy::Shift;
    ///
    /// assert!(Shift::down().is_down());
    /// ```
    #[must_use]
    pub fn is_down(self) -> bool {
        self.down
    }

    /// Returns whether left movement is enabled.
    ///
    /// ```
    /// use idm::diy::Shift;
    ///
    /// assert!(Shift::left().is_left());
    /// ```
    #[must_use]
    pub fn is_left(self) -> bool {
        self.left
    }

    /// Returns whether right movement is enabled.
    ///
    /// ```
    /// use idm::diy::Shift;
    ///
    /// assert!(Shift::right().is_right());
    /// ```
    #[must_use]
    pub fn is_right(self) -> bool {
        self.right
    }

    #[must_use]
    fn any(self) -> bool {
        self.up || self.down || self.left || self.right
    }

    #[must_use]
    fn as_payload(self) -> [u8; 4] {
        [
            u8::from(self.up),
            u8::from(self.down),
            u8::from(self.left),
            u8::from(self.right),
        ]
    }
}

impl BitOr for Shift {
    type Output = Self;

    /// Combines two shifts so that each enabled direction is preserved.
    ///
    /// ```
    /// use idm::diy::Shift;
    ///
    /// let diagonal = Shift::up() | Shift::left();
    /// assert!(diagonal.is_up());
    /// assert!(diagonal.is_left());
    /// assert!(!diagonal.is_down());
    /// ```
    fn bitor(self, rhs: Self) -> Self {
        Self {
            up: self.up || rhs.up,
            down: self.down || rhs.down,
            left: self.left || rhs.left,
            right: self.right || rhs.right,
        }
    }
}

/// Shared state owned by draw and movement handles.
struct HandleState {
    _mode_guard: DiyModeGuard,
    session: DeviceSession,
}

/// Handle for draw-mode commands.
///
/// Entering DIY mode and obtaining a handle:
///
/// ```
/// # async fn demo(session: idm::DeviceSession) -> Result<(), idm::ProtocolError> {
/// let mut draw = idm::diy::DrawHandle::open(&session).await?;
/// let points = [idm::diy::Point::new(0, 0)];
/// draw.set_pixels(idm::Rgb::new(255, 0, 0), &points).await?;
/// # Ok(())
/// # }
/// ```
pub struct DrawHandle {
    state: HandleState,
}

impl DrawHandle {
    /// Enters DIY mode and returns a draw handle.
    ///
    /// The device exits DIY mode when the last handle produced from this one
    /// is dropped.
    ///
    /// # Errors
    ///
    /// Returns an error when entering DIY mode fails.
    pub async fn open(session: &DeviceSession) -> Result<Self, ProtocolError> {
        let mode_guard = DiyModeGuard::enter(session).await?;
        Ok(Self {
            state: HandleState {
                _mode_guard: mode_guard,
                session: session.clone(),
            },
        })
    }

    /// Switches to movement mode, consuming this handle.
    ///
    /// ```
    /// # async fn demo(session: idm::DeviceSession) -> Result<(), idm::ProtocolError> {
    /// let draw = idm::diy::DrawHandle::open(&session).await?;
    /// let _movement = draw.into_movement();
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn into_movement(self) -> MovementHandle {
        MovementHandle { state: self.state }
    }

    /// Draws one pixel.
    ///
    /// ```
    /// # async fn demo(session: idm::DeviceSession) -> Result<(), idm::ProtocolError> {
    /// let mut draw = idm::diy::DrawHandle::open(&session).await?;
    /// draw.set_pixel(idm::Rgb::new(255, 0, 0), idm::diy::Point::new(0, 0)).await?;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// Returns an error when the point is invalid or the BLE write fails.
    pub async fn set_pixel(
        &mut self,
        colour: Rgb,
        point: Point,
    ) -> Result<CommandStats, ProtocolError> {
        self.send_points(RuntimeMode::NoEffect, colour, &[point])
            .await
    }

    /// Draws multiple pixels with one colour.
    ///
    /// ```
    /// # async fn demo(session: idm::DeviceSession) -> Result<(), idm::ProtocolError> {
    /// let mut draw = idm::diy::DrawHandle::open(&session).await?;
    /// let points = [idm::diy::Point::new(0, 0), idm::diy::Point::new(1, 0)];
    /// draw.set_pixels(idm::Rgb::new(0, 255, 0), &points).await?;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// Returns an error when points are invalid or the BLE write fails.
    pub async fn set_pixels(
        &mut self,
        colour: Rgb,
        points: &[Point],
    ) -> Result<CommandStats, ProtocolError> {
        self.send_points(RuntimeMode::NoEffect, colour, points)
            .await
    }

    /// Draws pixels mirrored across the vertical axis.
    ///
    /// ```
    /// # async fn demo(session: idm::DeviceSession) -> Result<(), idm::ProtocolError> {
    /// let mut draw = idm::diy::DrawHandle::open(&session).await?;
    /// let points = [idm::diy::Point::new(2, 3)];
    /// draw.mirror_horizontal(idm::Rgb::new(255, 255, 0), &points).await?;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// Returns an error when points are invalid or the BLE write fails.
    pub async fn mirror_horizontal(
        &mut self,
        colour: Rgb,
        points: &[Point],
    ) -> Result<CommandStats, ProtocolError> {
        self.send_points(RuntimeMode::HorizontalMirror, colour, points)
            .await
    }

    /// Draws pixels mirrored across the horizontal axis.
    ///
    /// ```
    /// # async fn demo(session: idm::DeviceSession) -> Result<(), idm::ProtocolError> {
    /// let mut draw = idm::diy::DrawHandle::open(&session).await?;
    /// let points = [idm::diy::Point::new(2, 3)];
    /// draw.mirror_vertical(idm::Rgb::new(255, 255, 0), &points).await?;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// Returns an error when points are invalid or the BLE write fails.
    pub async fn mirror_vertical(
        &mut self,
        colour: Rgb,
        points: &[Point],
    ) -> Result<CommandStats, ProtocolError> {
        self.send_points(RuntimeMode::VerticalMirror, colour, points)
            .await
    }

    /// Erases pixels to black.
    ///
    /// ```
    /// # async fn demo(session: idm::DeviceSession) -> Result<(), idm::ProtocolError> {
    /// let mut draw = idm::diy::DrawHandle::open(&session).await?;
    /// let points = [idm::diy::Point::new(5, 5)];
    /// draw.erase_pixels(&points).await?;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// Returns an error when points are invalid or the BLE write fails.
    pub async fn erase_pixels(&mut self, points: &[Point]) -> Result<CommandStats, ProtocolError> {
        self.send_points(RuntimeMode::Erase, Rgb::new(0, 0, 0), points)
            .await
    }

    async fn send_points(
        &mut self,
        mode: RuntimeMode,
        colour: Rgb,
        points: &[Point],
    ) -> Result<CommandStats, ProtocolError> {
        validate_points(&self.state.session, points)?;

        let mut payload = Vec::with_capacity(3 + (points.len() * 2));
        payload.push(colour.r);
        payload.push(colour.g);
        payload.push(colour.b);
        for point in points {
            payload.push(point.x());
            payload.push(point.y());
        }

        let bytes_written =
            DiyHandler::send_runtime_command(&self.state.session, mode, &payload).await?;

        Ok(CommandStats { bytes_written })
    }
}

/// Handle for movement-mode commands.
pub struct MovementHandle {
    state: HandleState,
}

impl MovementHandle {
    /// Switches to draw mode, consuming this handle.
    ///
    /// ```
    /// # async fn demo(session: idm::DeviceSession) -> Result<(), idm::ProtocolError> {
    /// let draw = idm::diy::DrawHandle::open(&session).await?;
    /// let movement = draw.into_movement();
    /// let _draw = movement.into_draw();
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn into_draw(self) -> DrawHandle {
        DrawHandle { state: self.state }
    }

    /// Moves painted content by one step in one or more directions.
    ///
    /// ```
    /// # async fn demo(session: idm::DeviceSession) -> Result<(), idm::ProtocolError> {
    /// let draw = idm::diy::DrawHandle::open(&session).await?;
    /// let mut movement = draw.into_movement();
    /// movement.shift(idm::diy::Shift::right()).await?;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// Returns an error when no direction is selected or the BLE write fails.
    pub async fn shift(&mut self, shift: Shift) -> Result<CommandStats, ProtocolError> {
        if !shift.any() {
            return Err(DiyError::EmptyMovementDirection.into());
        }

        let bytes_written = DiyHandler::send_runtime_command(
            &self.state.session,
            RuntimeMode::OverallMovement,
            &shift.as_payload(),
        )
        .await?;

        Ok(CommandStats { bytes_written })
    }
}

/// Uploads one DIY RGB frame, entering and exiting DIY mode around the
/// transfer.
///
/// ```
/// # async fn demo(session: idm::DeviceSession, frame: idm::Rgb888Frame) -> Result<(), idm::ProtocolError> {
/// let request = idm::diy::UploadRequest::new(frame);
/// let _stats = idm::diy::upload(&session, request).await?;
/// # Ok(())
/// # }
/// ```
///
/// # Errors
///
/// Returns an error when validation, transfer encoding, BLE writes, or
/// transfer acknowledgements fail.
pub async fn upload(
    session: &DeviceSession,
    request: UploadRequest,
) -> Result<UploadStats, ProtocolError> {
    DiyHandler::upload(session, request).await
}

fn validate_points(session: &DeviceSession, points: &[Point]) -> Result<(), ProtocolError> {
    if points.is_empty() {
        return Err(DiyError::EmptyPointList.into());
    }

    let panel_dimensions = session
        .device_profile()
        .panel_dimensions()
        .ok_or(DiyError::MissingPanelDimensions)?;
    let width = panel_dimensions.width();
    let height = panel_dimensions.height();

    for point in points {
        if u16::from(point.x()) >= width || u16::from(point.y()) >= height {
            return Err(DiyError::PointOutOfBounds {
                x: point.x(),
                y: point.y(),
                panel_dimensions,
            }
            .into());
        }
    }

    Ok(())
}
