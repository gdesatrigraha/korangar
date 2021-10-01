pub struct SmoothedValue {
    current: f32,
    desired: f32,
    threshhold: f32,
    speed: f32,
}

impl SmoothedValue {

    pub fn new(value: f32, threshhold: f32, speed: f32) -> Self {
        let current = value;
        let desired = value;
        return Self { current, desired, threshhold, speed };
    }

    pub fn update(&mut self, delta_time: f64) {
        if self.desired >= self.current + self.threshhold {
            let new_current = self.current + ((self.desired - self.current).sqrt() * self.speed * delta_time as f32);
            self.current = self.desired.min(new_current);
        } else if self.desired <= self.current - self.threshhold {
            let new_current = self.current - ((self.current - self.desired).sqrt() * self.speed * delta_time as f32);
            self.current = self.desired.max(new_current);
        }
    }

    pub fn move_desired(&mut self, offset: f32) {
        self.desired += offset;
    }

    pub fn get_current(&self) -> f32 {
        return self.current;
    }
}
